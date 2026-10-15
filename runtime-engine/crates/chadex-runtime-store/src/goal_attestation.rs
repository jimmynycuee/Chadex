//! External authority is separate from executable success. Only a detached
//! signature under a locally configured, project/owner-scoped public key may
//! authorize an external gate. The runtime never owns a signing key.
use crate::{GoalAcceptanceReceiptObservation, GoalRunRecord, AcceptanceCheck};
use serde::{Deserialize,Serialize};

#[derive(Debug,Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TrustedGateKey {
    pub owner:String,
    pub project_root:String,
    pub runner_client_id:String,
    pub public_key_hex:String,
}
#[derive(Debug,Clone,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
struct Statement {
    goal_run_id:String,
    acceptance_revision:i64,
    checkpoint_sha:String,
    check_name:String,
    argv_sha256:String,
    output_sha256:String,
    artifact_sha256:String,
    owner:String,
    action:String,
    exit_code:i32,
    not_before_unix_ms:i64,
    expires_at_unix_ms:i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    statement:Statement,
    signature_hex:String,
    /// Computed independently by a fixed Runner reader, never copied from statement.
    observed_artifact_sha256:String,
}
fn bytes<const N:usize>(text:&str)->Option<[u8;N]>{
    if text.len()!=N*2 || !text.bytes().all(|b|b.is_ascii_digit() || (b'a'..=b'f').contains(&b)){return None}
    let mut out=[0;N];
    for (i,b) in out.iter_mut().enumerate(){*b=u8::from_str_radix(&text[i*2..i*2+2],16).ok()?}
    Some(out)
}
fn signing_bytes(statement:&Statement)->Vec<u8>{
    let mut bytes=b"chadex.external-gate.v1\0".to_vec();
    bytes.extend(serde_json::to_vec(statement).expect("fixed statement serializes"));bytes
}
pub(crate) fn configured_authorization(
    run:&GoalRunRecord,check:&AcceptanceCheck,observation:&GoalAcceptanceReceiptObservation,
)->bool{
    let keys=std::env::var("CHADEX_GOAL_EXTERNAL_ATTESTATION_KEYS").ok()
        .and_then(|raw|serde_json::from_str::<Vec<TrustedGateKey>>(&raw).ok()).unwrap_or_default();
    verify(run,check,observation,&keys)
}
pub(crate) fn verify(run:&GoalRunRecord,check:&AcceptanceCheck,observation:&GoalAcceptanceReceiptObservation,keys:&[TrustedGateKey])->bool{
    let Some(manifest)=check.external_gate.as_ref() else{return false};
    let Some(value)=observation.external_attestation.clone() else{return false};
    let Ok(envelope)=serde_json::from_value::<Envelope>(value) else{return false};
    let s=&envelope.statement;
    if s.goal_run_id!=run.goal_run_id || s.acceptance_revision!=observation.acceptance_revision
        || s.checkpoint_sha!=observation.checkpoint_sha || s.check_name!=check.name
        || s.argv_sha256!=observation.argv_sha256 || s.output_sha256!=observation.output_sha256
        || s.owner!=manifest.owner || s.action!=manifest.action || s.exit_code!=0
        || observation.exit_code!=Some(0) || observation.timed_out
        || bytes::<32>(&s.artifact_sha256).is_none() || s.artifact_sha256!=envelope.observed_artifact_sha256
        || s.not_before_unix_ms>observation.observed_at_unix_ms
        || s.expires_at_unix_ms<=observation.observed_at_unix_ms
        || s.expires_at_unix_ms>s.not_before_unix_ms.saturating_add(24*60*60*1000)
    {return false}
    let Some(signature)=bytes::<64>(&envelope.signature_hex) else{return false};
    let payload=signing_bytes(s);
    keys.iter().filter(|key|key.owner==manifest.owner && key.project_root==run.binding.project_root && key.runner_client_id==run.binding.runner_client_id)
        .filter_map(|key|bytes::<32>(&key.public_key_hex))
        .any(|public|ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519,public).verify(&payload,&signature).is_ok())
}

#[cfg(test)]
pub(crate) fn test_signing_bytes(value:serde_json::Value)->Vec<u8>{
    signing_bytes(&serde_json::from_value::<Statement>(value).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair,KeyPair};
    #[test]
    fn detached_signature_domain_and_payload_are_not_self_reported(){
        let pair=Ed25519KeyPair::from_seed_unchecked(&[7u8;32]).unwrap();
        let statement=Statement{goal_run_id:"synthetic-only".into(),acceptance_revision:2,checkpoint_sha:"a".repeat(40),check_name:"gate".into(),argv_sha256:"b".repeat(64),output_sha256:"c".repeat(64),artifact_sha256:"d".repeat(64),owner:"fixture".into(),action:"witness".into(),exit_code:0,not_before_unix_ms:1,expires_at_unix_ms:100};
        let payload=signing_bytes(&statement);let signature=pair.sign(&payload);
        let key=ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519,pair.public_key().as_ref());
        assert!(key.verify(&payload,signature.as_ref()).is_ok());
        assert!(key.verify(&serde_json::to_vec(&statement).unwrap(),signature.as_ref()).is_err());
        let mut changed=statement.clone();changed.acceptance_revision+=1;
        assert!(key.verify(&signing_bytes(&changed),signature.as_ref()).is_err());
        assert!(bytes::<32>(&"z".repeat(64)).is_none());
    }
}

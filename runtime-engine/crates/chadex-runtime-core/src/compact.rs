//! Low-level opaque encoding only. Domains choose identity/proof strength and
//! own collision checks: 12 random bytes for identities, >=16 for proofs.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).expect("operating system CSPRNG unavailable");
    bytes
}

pub fn encode(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn random_suffix<const N: usize>() -> String {
    encode(random_bytes::<N>())
}

/// Exact decoded size and canonical pad bits/alphabet; no alternate spellings.
pub fn decode<const N: usize>(value: &str) -> Option<[u8; N]> {
    let bytes = URL_SAFE_NO_PAD.decode(value).ok()?;
    if encode(&bytes) != value {
        return None;
    }
    bytes.try_into().ok()
}

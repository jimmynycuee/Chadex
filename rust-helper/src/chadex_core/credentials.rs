use super::{ChadexError, ChadexResult};
use zeroize::Zeroizing;

#[derive(Default)]
pub struct CredentialStore {
    tunnel_id: Option<String>,
    api_key: Option<Zeroizing<String>>,
}

impl CredentialStore {
    pub fn configured(&self) -> bool {
        self.tunnel_id.is_some() && self.api_key.is_some()
    }

    pub fn tunnel_id(&self) -> Option<String> {
        self.tunnel_id.clone()
    }

    pub fn provide(&mut self, tunnel_id: &str, api_key: Zeroizing<String>) -> ChadexResult<()> {
        let tunnel_id = tunnel_id.trim();
        validate_tunnel_id(tunnel_id)?;
        validate_api_key(api_key.as_str())?;
        self.tunnel_id = Some(tunnel_id.to_string());
        self.api_key = Some(api_key);
        Ok(())
    }

    pub fn clear(&mut self) {
        self.tunnel_id = None;
        self.api_key = None;
    }

    pub fn for_start(&self) -> ChadexResult<(String, Zeroizing<String>)> {
        let tunnel_id = self.tunnel_id.clone().ok_or_else(config_error)?;
        let api_key = self
            .api_key
            .as_ref()
            .map(|key| Zeroizing::new(key.to_string()))
            .ok_or_else(config_error)?;
        Ok((tunnel_id, api_key))
    }
}

pub fn validate_tunnel_id(value: &str) -> ChadexResult<()> {
    let valid = value.strip_prefix("tunnel_").is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if valid {
        Ok(())
    } else {
        Err(ChadexError::new(
            "tunnel_config_invalid",
            "Tunnel ID is invalid",
            "Use the OpenAI Tunnel ID in the form tunnel_ followed by 32 lowercase hexadecimal characters.",
        ))
    }
}

pub fn validate_api_key(value: &str) -> ChadexResult<()> {
    if !value.is_empty() && value.len() <= 8192 && value.bytes().all(|byte| byte.is_ascii_graphic())
    {
        Ok(())
    } else {
        Err(config_error())
    }
}

fn config_error() -> ChadexError {
    ChadexError::new(
        "tunnel_config_invalid",
        "Secure MCP Tunnel credentials are missing or invalid",
        "Enter a valid Tunnel ID and restricted OpenAI API key, then retry.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_store_is_memory_only_and_clearable() {
        let mut store = CredentialStore::default();
        store
            .provide(
                "tunnel_6aa2855405988191b439e7c0860a51c7",
                Zeroizing::new("sk-test-only".to_string()),
            )
            .unwrap();
        assert!(store.configured());
        assert_eq!(
            store.tunnel_id().as_deref(),
            Some("tunnel_6aa2855405988191b439e7c0860a51c7")
        );
        store.clear();
        assert!(!store.configured());
        assert!(store.for_start().is_err());
    }

    #[test]
    fn tunnel_id_validation_is_strict() {
        assert!(validate_tunnel_id("tunnel_6aa2855405988191b439e7c0860a51c7").is_ok());
        assert!(validate_tunnel_id("tunnel_BAD").is_err());
        assert!(validate_tunnel_id("6aa2855405988191b439e7c0860a51c7").is_err());
    }
}

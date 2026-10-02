use zeroize::Zeroizing;

const SERVICE: &str = "app.chadex.windows.credentials";
const ACCOUNT: &str = "openai-tunnel-api-key";

#[cfg(windows)]
fn entry() -> Result<keyring::Entry, String> {
    let mut service = SERVICE.to_owned();
    #[cfg(feature = "desktop-smoke")]
    if std::env::var_os("CHADEX_DESKTOP_SMOKE_DIR").is_some() {
        service = format!("{SERVICE}.smoke.{}", std::process::id());
    }
    keyring::Entry::new(&service, ACCOUNT).map_err(|_| "credential_store_unavailable".into())
}

#[cfg(windows)]
pub fn read() -> Result<Option<Zeroizing<String>>, String> {
    match entry()?.get_password() {
        Ok(value) => Ok(Some(Zeroizing::new(value))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err("credential_read_failed".into()),
    }
}

#[cfg(windows)]
pub fn write(value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 2400 {
        return Err("credential_invalid".into());
    }
    entry()?
        .set_password(value)
        .map_err(|_| "credential_write_failed".into())
}

#[cfg(windows)]
pub fn delete() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err("credential_delete_failed".into()),
    }
}

// This target is a Windows product. Compiling it on macOS must not access or
// change the native SwiftUI application's Keychain entries.
#[cfg(not(windows))]
pub fn read() -> Result<Option<Zeroizing<String>>, String> {
    let _ = (SERVICE, ACCOUNT);
    Ok(None)
}
#[cfg(not(windows))]
pub fn write(_: &str) -> Result<(), String> {
    Err("windows_credential_store_required".into())
}
#[cfg(not(windows))]
pub fn delete() -> Result<(), String> {
    Err("windows_credential_store_required".into())
}

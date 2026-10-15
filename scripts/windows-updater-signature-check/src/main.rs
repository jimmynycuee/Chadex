use base64::{engine::general_purpose::STANDARD, Engine as _};
use minisign_verify::{PublicKey, Signature};
use std::{env, fs, path::PathBuf, process};

fn verify_bytes(public_key: &str, signature_text: &str, archive: &[u8]) -> Result<(), &'static str> {
    let public_key_bytes = STANDARD
        .decode(public_key.trim())
        .map_err(|_| "public_key_invalid")?;
    let public_key_text = std::str::from_utf8(&public_key_bytes).map_err(|_| "public_key_invalid")?;
    let public_key = PublicKey::decode(public_key_text).map_err(|_| "public_key_invalid")?;
    let signature_bytes = STANDARD
        .decode(signature_text.trim())
        .map_err(|_| "signature_invalid")?;
    let signature_text = std::str::from_utf8(&signature_bytes).map_err(|_| "signature_invalid")?;
    let signature = Signature::decode(signature_text).map_err(|_| "signature_invalid")?;
    public_key
        .verify(archive, &signature, true)
        .map_err(|_| "signature_public_key_mismatch")
}

fn verify() -> Result<(), &'static str> {
    let mut args = env::args_os().skip(1);
    let mut signature_path: Option<PathBuf> = None;
    let mut archive_path: Option<PathBuf> = None;
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--signature-file") => signature_path = args.next().map(PathBuf::from),
            Some("--archive") => archive_path = args.next().map(PathBuf::from),
            _ => return Err("arguments_invalid"),
        }
    }

    let public_key = env::var("CHADEX_UPDATER_PUBLIC_KEY").map_err(|_| "public_key_missing")?;
    let signature_path = signature_path.ok_or("signature_file_missing")?;
    let signature_text = fs::read_to_string(signature_path).map_err(|_| "signature_file_invalid")?;
    let archive_path = archive_path.ok_or("archive_missing")?;
    let archive = fs::read(archive_path).map_err(|_| "archive_invalid")?;
    verify_bytes(&public_key, &signature_text, &archive)
}

fn main() {
    if let Err(code) = verify() {
        eprintln!("updater_signature_verification_failed: {code}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::verify_bytes;

    const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEJDNzQwRTY5MEYzNzI1OUUKUldTZUpUY1BhUTUwdk5hUUxkaDkwSHRvNjlqOGpvOGZQR2xsVHpzWHFrN3QvdjhhdVFmMjZYejIK";
    const OTHER_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEM1N0IyRThFQTQ2NEJCOTUKUldTVnUyU2tqaTU3eFhXemZmcXNPODFQeXJPcWd5OERXaWZVZmMrWFo2cUhBYVpRRjlBYUFudzEK";
    const SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTZUpUY1BhUTUwdk5wNXd6RzNFNk9LbVJ3SHRzVDdiYnNZNkZpd2Y4RVVXanVPTWVBbWR5T3JJY2daTEZ1QUFtTjlxVVU4MnhOZ0R1SnZoNjJRMmJZRzMvbXRSbnc2UmdnPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkyMDg3NjY4CWZpbGU6Zml4dHVyZS5uc2lzLnppcAl2ZXJzaW9uOjAuNy4yCjZnOXhyd3p3RVR3NHFwZ2FCL0xTT0J5RlhSTGl3MTZvS2E3ZXlFYmJPdGh3N0xUWWcyTWZvZUhkQ3ZlYzNWVmFxMGh2ZVJBSzdaekFIWks3S1ZQV0JRPT0K";
    const ARCHIVE: &[u8] = b"Chadex Tauri updater signature fixture v1";

    #[test]
    fn verifies_signature_for_exact_archive_bytes() {
        assert_eq!(verify_bytes(PUBLIC_KEY, SIGNATURE, ARCHIVE), Ok(()));
        assert_eq!(
            verify_bytes(PUBLIC_KEY, SIGNATURE, b"tampered updater archive"),
            Err("signature_public_key_mismatch")
        );
    }

    #[test]
    fn rejects_a_mismatched_public_key() {
        assert_eq!(
            verify_bytes(OTHER_PUBLIC_KEY, SIGNATURE, ARCHIVE),
            Err("signature_public_key_mismatch")
        );
    }
}

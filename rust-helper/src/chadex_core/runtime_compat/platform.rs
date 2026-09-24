use crate::chadex_core::runtime_compat::error::{DesktopError, DesktopResult};
use crate::chadex_core::runtime_compat::models::PowerShellRuntimeSnapshot;
use chadex_runtime_process::SpawnOptions;

#[cfg(target_os = "windows")]
const POWERSHELL_INSTALL_GUIDE_URL: &str =
    "https://learn.microsoft.com/powershell/scripting/install/install-powershell-on-windows";

pub fn managed_spawn_options(silent_child_breakaway: bool) -> SpawnOptions {
    #[cfg(target_os = "windows")]
    {
        return windows::managed_spawn_options(silent_child_breakaway);
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = silent_child_breakaway;
        SpawnOptions::new()
    }
}

pub fn powershell_runtime_snapshot() -> Option<PowerShellRuntimeSnapshot> {
    #[cfg(target_os = "windows")]
    {
        return Some(windows::powershell_runtime_snapshot());
    }

    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

pub fn open_powershell_install_guide() -> DesktopResult<()> {
    #[cfg(target_os = "windows")]
    {
        return windows::open_external_url(POWERSHELL_INSTALL_GUIDE_URL).map_err(|error| {
            DesktopError::new(
                "powershell_install_guide_unavailable",
                format!("failed to open the PowerShell 7 installation guide: {error}"),
                "Open the Microsoft PowerShell installation documentation in your browser.",
            )
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err(DesktopError::new(
            "powershell_install_guide_unsupported",
            "PowerShell 7 installation guidance is only shown on Windows",
            "No PowerShell installation is required on this platform.",
        ))
    }
}

pub fn current_username() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .ok()
        .map(|value| normalize_local_username(&value))
        .unwrap_or_else(|| "desktop".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsernameToken {
    Original(char),
    Separator,
}

fn normalize_local_username(value: &str) -> String {
    let trimmed = value.trim();
    if server_accepts_username(trimmed) {
        return trimmed.to_string();
    }

    let mut tokens = Vec::with_capacity(trimmed.len().min(64));
    for character in trimmed.chars() {
        let character = character.to_ascii_lowercase();
        if username_character_allowed(character) {
            tokens.push(UsernameToken::Original(character));
        } else if tokens.last() != Some(&UsernameToken::Separator) {
            tokens.push(UsernameToken::Separator);
        }
    }

    while tokens.first() == Some(&UsernameToken::Separator) {
        tokens.remove(0);
    }
    while tokens.last() == Some(&UsernameToken::Separator) {
        tokens.pop();
    }

    tokens.truncate(64);
    while tokens.last() == Some(&UsernameToken::Separator) {
        tokens.pop();
    }

    let normalized: String = tokens
        .into_iter()
        .map(|token| match token {
            UsernameToken::Original(character) => character,
            UsernameToken::Separator => '-',
        })
        .collect();

    if normalized.is_empty() {
        "desktop".to_string()
    } else {
        normalized
    }
}

fn server_accepts_username(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= 64
        && value.chars().all(username_character_allowed)
}

fn username_character_allowed(character: char) -> bool {
    character.is_ascii_lowercase() || character.is_ascii_digit() || matches!(character, '_' | '-')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemProxyCandidate {
    pub url: String,
    pub enabled: bool,
}

pub fn system_http_proxy_candidate() -> Option<SystemProxyCandidate> {
    #[cfg(target_os = "windows")]
    {
        return windows::system_http_proxy_candidate();
    }

    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

pub(crate) fn normalize_proxy_server(value: &str) -> Option<String> {
    let selected = select_proxy_target(value.trim())?;
    let candidate = if selected.contains("://") {
        selected.to_string()
    } else {
        format!("http://{selected}")
    };

    let parsed = url::Url::parse(&candidate).ok()?;
    let safe_origin = matches!(parsed.scheme(), "http" | "https")
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.host_str().is_some()
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && matches!(parsed.path(), "" | "/");

    safe_origin.then(|| candidate.trim_end_matches('/').to_string())
}

fn select_proxy_target(value: &str) -> Option<&str> {
    if value.is_empty() {
        return None;
    }
    if !value.contains('=') {
        return Some(value);
    }

    let mut http = None;
    let mut https = None;
    for entry in value.split(';') {
        let Some((scheme, target)) = entry.split_once('=') else {
            continue;
        };
        let scheme = scheme.trim();
        let target = target.trim();
        if scheme.eq_ignore_ascii_case("https") {
            https = Some(target);
        } else if scheme.eq_ignore_ascii_case("http") {
            http = Some(target);
        }
    }
    https.or(http)
}

pub(crate) fn proxy_is_loopback(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(loopback_host))
        == Some(true)
}

fn loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]"
}

#[cfg(target_os = "windows")]
mod windows {
    use super::{normalize_proxy_server, SystemProxyCandidate};
    use crate::chadex_core::runtime_compat::models::PowerShellRuntimeSnapshot;
    use std::ffi::OsStr;
    use std::io;
    use std::path::PathBuf;
    use std::process::Command;
    use chadex_runtime_process::SpawnOptions;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub(super) fn managed_spawn_options(silent_child_breakaway: bool) -> SpawnOptions {
        SpawnOptions {
            windows_creation_flags: CREATE_NO_WINDOW,
            windows_silent_child_breakaway: silent_child_breakaway,
        }
    }

    pub(super) fn powershell_runtime_snapshot() -> PowerShellRuntimeSnapshot {
        let path = std::env::var_os("PATH");
        PowerShellRuntimeSnapshot {
            pwsh_available: program_on_path("pwsh.exe", path.as_deref()),
            windows_powershell_available: program_on_path("powershell.exe", path.as_deref())
                || std::env::var_os("SystemRoot").is_some_and(|system_root| {
                    PathBuf::from(system_root)
                        .join("System32")
                        .join("WindowsPowerShell")
                        .join("v1.0")
                        .join("powershell.exe")
                        .is_file()
                }),
        }
    }

    fn program_on_path(program: &str, path: Option<&OsStr>) -> bool {
        path.into_iter()
            .flat_map(std::env::split_paths)
            .any(|directory| directory.join(program).is_file())
    }

    pub(super) fn open_external_url(url: &str) -> io::Result<()> {
        Command::new("explorer.exe").arg(url).spawn().map(|_| ())
    }

    pub(super) fn system_http_proxy_candidate() -> Option<SystemProxyCandidate> {
        let internet_settings = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
            .ok()?;
        let raw: String = internet_settings.get_value("ProxyServer").ok()?;
        let url = normalize_proxy_server(&raw)?;
        let enabled = internet_settings
            .get_value::<u32, _>("ProxyEnable")
            .ok()
            .is_some_and(|value| value != 0);
        Some(SystemProxyCandidate { url, enabled })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn path_probe_does_not_execute_programs() {
            let root = tempfile::tempdir().unwrap();
            let path = std::env::join_paths([root.path()]).unwrap();
            assert!(!program_on_path("pwsh.exe", Some(path.as_os_str())));
            std::fs::write(root.path().join("pwsh.exe"), b"").unwrap();
            assert!(program_on_path("pwsh.exe", Some(path.as_os_str())));
        }

        #[test]
        fn child_breakaway_is_opt_in() {
            let ordinary = managed_spawn_options(false);
            assert_eq!(ordinary.windows_creation_flags, CREATE_NO_WINDOW);
            assert!(!ordinary.windows_silent_child_breakaway);

            let supervisor = managed_spawn_options(true);
            assert_eq!(supervisor.windows_creation_flags, CREATE_NO_WINDOW);
            assert!(supervisor.windows_silent_child_breakaway);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_normalization_matches_runtime_contract() {
        for (input, expected) in [
            ("alice", "alice"),
            ("alice--dev", "alice--dev"),
            ("alice_dev", "alice_dev"),
            ("alice-", "alice-"),
            ("-alice", "-alice"),
            ("Alice", "alice"),
            ("  alice\t", "alice"),
            ("Alice Smith", "alice-smith"),
            ("Alice  Smith", "alice-smith"),
            ("alice.smith", "alice-smith"),
            ("DOMAIN\\Jane Doe", "domain-jane-doe"),
            ("alice..dev", "alice-dev"),
            ("alice.-dev", "alice--dev"),
            ("alice-.", "alice-"),
            ("用户", "desktop"),
            ("", "desktop"),
            (" \t ", "desktop"),
        ] {
            assert_eq!(normalize_local_username(input), expected);
        }

        assert_eq!(normalize_local_username(&"A".repeat(65)), "a".repeat(64));
    }

    #[test]
    fn valid_usernames_are_not_rewritten() {
        for name in [
            "alice",
            "alice--dev",
            "alice-",
            "-alice",
            "_",
            "--",
            "user_123",
        ] {
            assert_eq!(normalize_local_username(name), name);
        }

        let maximum_length = "a".repeat(64);
        assert_eq!(normalize_local_username(&maximum_length), maximum_length);
    }

    #[test]
    fn proxy_normalization_accepts_supported_shapes() {
        assert_eq!(
            normalize_proxy_server("127.0.0.1:7890").as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(
            normalize_proxy_server("http=127.0.0.1:7890;https=127.0.0.1:7890").as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(
            normalize_proxy_server("http=127.0.0.1:7890;https=127.0.0.1:7891").as_deref(),
            Some("http://127.0.0.1:7891")
        );
        assert!(proxy_is_loopback("http://127.0.0.1:7890"));
        assert!(proxy_is_loopback("http://localhost:7890"));
        assert!(proxy_is_loopback("http://[::1]:7890"));
        assert!(!proxy_is_loopback("http://proxy.example.test:8080"));
    }

    #[test]
    fn proxy_normalization_rejects_unsafe_origins() {
        for value in [
            "http://user:secret@127.0.0.1:7890",
            "socks5://127.0.0.1:7890",
            "http://proxy.example.test/path",
            "http://proxy.example.test?query=yes",
            "http://proxy.example.test#fragment",
        ] {
            assert!(normalize_proxy_server(value).is_none(), "{value}");
        }
    }

    #[test]
    fn non_windows_platform_has_no_powershell_runtime_snapshot() {
        #[cfg(not(target_os = "windows"))]
        assert!(powershell_runtime_snapshot().is_none());
    }
}

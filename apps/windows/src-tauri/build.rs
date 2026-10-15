fn main() {
    println!("cargo:rerun-if-env-changed=CHADEX_UPDATER_PUBLIC_KEY");
    println!("cargo:rerun-if-env-changed=CHADEX_REQUIRE_UPDATER_KEY");
    if std::env::var("CHADEX_REQUIRE_UPDATER_KEY").as_deref() == Ok("1") {
        let key = std::env::var("CHADEX_UPDATER_PUBLIC_KEY").unwrap_or_default();
        if key.trim().is_empty() || key == "UNCONFIGURED_UPDATER_PUBLIC_KEY" {
            panic!("A configured Tauri updater public key is required for a signed release build.");
        }
    }
    tauri_build::build()
}

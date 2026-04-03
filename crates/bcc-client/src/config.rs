use std::path::PathBuf;

/// Client configuration with sensible defaults.
///
/// All fields can be overridden via CLI flags; this struct provides fallback values.
/// No file I/O or env-var framework is needed — `HOME`/`APPDATA` are read directly.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Default path to the keystore file.
    pub keystore_path: PathBuf,
    /// Default node HTTP base URL (no trailing slash).
    pub node_url: String,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            keystore_path: default_keystore_path(),
            node_url: std::env::var("RPC_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string()),
        }
    }
}

/// Returns the platform-appropriate default keystore path.
///
/// Resolution order:
/// 1. `$HOME/.bcc/keystore.json` (Unix)
/// 2. `%APPDATA%\bcc\keystore.json` (Windows)
/// 3. `./keystore.json` (fallback when neither env var is set)
///
/// The `dirs` crate is intentionally not used to avoid an extra dependency.
pub fn default_keystore_path() -> PathBuf {
    bcc_dir().join("keystore.json")
}

fn bcc_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".bcc");
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata).join("bcc");
    }
    PathBuf::from(".")
}

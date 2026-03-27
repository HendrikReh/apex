//! Chunking configuration loaded from `config/app.toml`.
//!
//! Settings are loaded once at startup and cached for the process lifetime.
//! Override the config file path with `CHUNKING_CONFIG_PATH` or `APP_CONFIG_PATH`.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use serde::Deserialize;

/// Runtime chunking configuration.
///
/// Loaded from the `[chunking]` section of `config/app.toml`.
/// Missing fields use baked-in defaults.
#[derive(Debug, Clone, Deserialize)]
pub struct ChunkingSettings {
    #[serde(default = "default_semantic_model")]
    pub semantic_model: String,
    #[serde(default = "default_max_tokens")]
    pub default_max_tokens: usize,
    #[serde(default = "default_overlap_ratio")]
    pub default_overlap_ratio: f32,
    #[serde(default = "default_semantic_max_chars")]
    pub semantic_max_chars: usize,
    #[serde(default = "default_semantic_timeout_secs")]
    pub semantic_timeout_secs: u64,
    #[serde(default = "default_semantic_enabled")]
    pub semantic_enabled: bool,
    #[serde(default = "default_semantic_concurrency")]
    pub semantic_concurrency: usize,
    #[serde(default = "default_cjk_token_multiplier")]
    pub cjk_token_multiplier: f32,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FileSettings {
    chunking: Option<ChunkingSettings>,
}

impl Default for ChunkingSettings {
    fn default() -> Self {
        Self {
            semantic_model: default_semantic_model(),
            default_max_tokens: default_max_tokens(),
            default_overlap_ratio: default_overlap_ratio(),
            semantic_max_chars: default_semantic_max_chars(),
            semantic_timeout_secs: default_semantic_timeout_secs(),
            semantic_enabled: default_semantic_enabled(),
            semantic_concurrency: default_semantic_concurrency(),
            cjk_token_multiplier: default_cjk_token_multiplier(),
        }
    }
}

/// Returns the process-wide chunking settings (loaded once, then cached).
pub fn settings() -> ChunkingSettings {
    static SETTINGS: OnceLock<ChunkingSettings> = OnceLock::new();
    SETTINGS.get_or_init(load_settings).clone()
}

pub(crate) fn resolve_config_relative_path(relative_path: &Path) -> PathBuf {
    if relative_path.is_absolute() {
        return relative_path.to_path_buf();
    }
    resolve_config_relative_path_from(&config_path(), relative_path)
}

pub(crate) fn resolve_config_relative_path_from(
    config_path: &Path,
    relative_path: &Path,
) -> PathBuf {
    if relative_path.is_absolute() {
        return relative_path.to_path_buf();
    }
    config_path
        .parent()
        .map(|dir| dir.join(relative_path))
        .unwrap_or_else(|| PathBuf::from("config").join(relative_path))
}

fn load_settings() -> ChunkingSettings {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(content) => match toml::from_str::<FileSettings>(&content) {
            Ok(cfg) => {
                if let Some(chunking) = cfg.chunking {
                    return chunking;
                }
                tracing::info!(
                    "Chunking config missing [chunking] section in {}, using baked defaults",
                    path.display()
                );
            }
            Err(err) => {
                tracing::warn!(
                    "Chunking config parse failed in {}: {}, using baked defaults",
                    path.display(),
                    err
                );
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!("Chunking config not found at {}, using baked defaults", path.display());
        }
        Err(err) => {
            tracing::warn!(
                "Failed reading chunking config at {}: {}, using baked defaults",
                path.display(),
                err
            );
        }
    }
    ChunkingSettings::default()
}

fn config_path() -> PathBuf {
    std::env::var("CHUNKING_CONFIG_PATH")
        .or_else(|_| std::env::var("APP_CONFIG_PATH"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config/app.toml"))
}

fn default_semantic_model() -> String {
    "unset".to_string()
}

fn default_max_tokens() -> usize {
    600
}

fn default_overlap_ratio() -> f32 {
    0.15
}

fn default_semantic_max_chars() -> usize {
    20_000
}

fn default_semantic_timeout_secs() -> u64 {
    60
}

fn default_semantic_enabled() -> bool {
    true
}

fn default_semantic_concurrency() -> usize {
    2
}

fn default_cjk_token_multiplier() -> f32 {
    1.5
}

#[cfg(test)]
mod tests {
    #![allow(unsafe_code)]
    use super::*;
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};
    use tempfile::NamedTempFile;

    static ENV_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn set_env_var(key: &str, value: &std::ffi::OsStr) {
        // SAFETY: test-only; restored via EnvGuard.
        unsafe { std::env::set_var(key, value) };
    }

    fn remove_env_var(key: &str) {
        // SAFETY: test-only; restored via EnvGuard.
        unsafe { std::env::remove_var(key) };
    }

    struct EnvGuard {
        prev_app: Option<String>,
        prev_chunk: Option<String>,
    }

    impl EnvGuard {
        fn new() -> Self {
            Self {
                prev_app: std::env::var("APP_CONFIG_PATH").ok(),
                prev_chunk: std::env::var("CHUNKING_CONFIG_PATH").ok(),
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev_app {
                Some(val) => set_env_var("APP_CONFIG_PATH", std::ffi::OsStr::new(val)),
                None => remove_env_var("APP_CONFIG_PATH"),
            }
            match &self.prev_chunk {
                Some(val) => set_env_var("CHUNKING_CONFIG_PATH", std::ffi::OsStr::new(val)),
                None => remove_env_var("CHUNKING_CONFIG_PATH"),
            }
        }
    }

    #[test]
    fn loads_from_file_with_default_fallback() {
        let _lock = env_lock();
        let _guard = EnvGuard::new();
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "[chunking]\nsemantic_model = \"foo\"\ndefault_max_tokens = 123\n\
             default_overlap_ratio = 0.2\nsemantic_max_chars = 9999\n\
             semantic_timeout_secs = 42\nsemantic_enabled = false\nsemantic_concurrency = 5"
        )
        .unwrap();
        set_env_var("APP_CONFIG_PATH", f.path().as_os_str());
        let cfg = load_settings();
        assert_eq!(cfg.semantic_model, "foo");
        assert_eq!(cfg.default_max_tokens, 123);
        assert!((cfg.default_overlap_ratio - 0.2).abs() < f32::EPSILON);
        assert_eq!(cfg.semantic_max_chars, 9_999);
        assert_eq!(cfg.semantic_timeout_secs, 42);
        assert!(!cfg.semantic_enabled);
        assert_eq!(cfg.semantic_concurrency, 5);
    }

    #[test]
    fn uses_neutral_semantic_model_when_omitted() {
        let _lock = env_lock();
        let _guard = EnvGuard::new();
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "[chunking]\ndefault_max_tokens = 256\ndefault_overlap_ratio = 0.1\n\
             semantic_max_chars = 12000\nsemantic_timeout_secs = 30\n\
             semantic_enabled = true\nsemantic_concurrency = 3"
        )
        .unwrap();
        set_env_var("APP_CONFIG_PATH", f.path().as_os_str());
        let cfg = load_settings();
        assert_eq!(cfg.semantic_model, "unset");
        assert_eq!(cfg.default_max_tokens, 256);
    }

    #[test]
    fn falls_back_to_default_when_missing() {
        let _lock = env_lock();
        let _guard = EnvGuard::new();
        remove_env_var("APP_CONFIG_PATH");
        remove_env_var("CHUNKING_CONFIG_PATH");
        let cfg = load_settings();
        assert_eq!(cfg.semantic_model, default_semantic_model());
        assert_eq!(cfg.default_max_tokens, default_max_tokens());
        assert!(cfg.semantic_enabled);
    }

    #[test]
    fn falls_back_to_defaults_when_chunking_section_missing() {
        let _lock = env_lock();
        let _guard = EnvGuard::new();
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "[other]\nfoo = \"bar\"").unwrap();
        set_env_var("APP_CONFIG_PATH", f.path().as_os_str());
        let cfg = load_settings();
        assert_eq!(cfg.default_max_tokens, default_max_tokens());
    }

    #[test]
    fn falls_back_to_defaults_when_toml_is_invalid() {
        let _lock = env_lock();
        let _guard = EnvGuard::new();
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "[chunking\nsemantic_model = \"foo\"").unwrap();
        set_env_var("APP_CONFIG_PATH", f.path().as_os_str());
        let cfg = load_settings();
        assert_eq!(cfg.default_max_tokens, default_max_tokens());
    }
}

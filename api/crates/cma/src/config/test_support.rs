use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use uuid::Uuid;

use super::*;

const TEST_API_KEY: &str = "sandbox-secret-never-print";
const TEST_IMAGE: &str = concat!(
    "registry.example/cma-runtime:v1@sha256:",
    "aaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaa"
);
const REQUIRED_RUNTIME_KEYS: [&str; 8] = [
    "CHRONO_SANDBOX_BASE_URL",
    "CHRONO_SANDBOX_API_KEY_FILE",
    "CHRONO_SANDBOX_IMAGE",
    "CHRONO_SANDBOX_NAMESPACE",
    "CHRONO_UPSTREAM_BASE_URL",
    "CHRONO_UPSTREAM_WIRE_API",
    "CHRONO_PLATFORM_BASE_URL",
    "CHRONO_DEPLOYMENT_ID",
];

struct TemporaryEntry {
    path: PathBuf,
}

impl TemporaryEntry {
    fn file(contents: &[u8]) -> Self {
        let entry = Self::missing();
        fs::write(&entry.path, contents).expect("write temporary configuration secret");
        entry
    }

    fn directory() -> Self {
        let entry = Self::missing();
        fs::create_dir(&entry.path).expect("create temporary configuration directory");
        entry
    }

    fn missing() -> Self {
        Self {
            path: std::env::temp_dir().join(format!("cma-config-{}", Uuid::new_v4())),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn path_str(&self) -> &str {
        self.path.to_str().expect("temporary path is Unicode")
    }
}

impl Drop for TemporaryEntry {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.path);
    }
}

fn configured(overrides: &[(&str, &str)]) -> Result<Config, String> {
    configured_with_removed_key(overrides, None)
}

fn configured_without(key: &str) -> Result<Config, String> {
    configured_with_removed_key(&[], Some(key))
}

fn configured_with_removed_key(
    overrides: &[(&str, &str)],
    removed_key: Option<&str>,
) -> Result<Config, String> {
    let secret = TemporaryEntry::file(format!("\n  {TEST_API_KEY}\t\n").as_bytes());
    let mut values = HashMap::from([
        ("CHRONO_AUTH_MODE".to_owned(), "trusted".to_owned()),
        (
            "CHRONO_MONGODB_URI".to_owned(),
            "mongodb://localhost/cma".to_owned(),
        ),
        (
            "CHRONO_AUTH_ENCRYPTION_KEY".to_owned(),
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned(),
        ),
        (
            "CHRONO_SANDBOX_BASE_URL".to_owned(),
            "http://sandbox.example".to_owned(),
        ),
        (
            "CHRONO_SANDBOX_API_KEY_FILE".to_owned(),
            secret.path().to_string_lossy().into_owned(),
        ),
        ("CHRONO_SANDBOX_IMAGE".to_owned(), TEST_IMAGE.to_owned()),
        (
            "CHRONO_SANDBOX_NAMESPACE".to_owned(),
            "chrono-bot".to_owned(),
        ),
        (
            "CHRONO_UPSTREAM_BASE_URL".to_owned(),
            "https://models.example/api/v1/proxy/s/model".to_owned(),
        ),
        (
            "CHRONO_UPSTREAM_WIRE_API".to_owned(),
            "responses".to_owned(),
        ),
        (
            "CHRONO_PLATFORM_BASE_URL".to_owned(),
            "https://platform.example".to_owned(),
        ),
        ("CHRONO_DEPLOYMENT_ID".to_owned(), "cma-test".to_owned()),
    ]);
    values.extend(
        overrides
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
    );
    if let Some(key) = removed_key {
        values.remove(key);
    }
    Config::from_lookup(|key| Ok(values.get(key).cloned()))
}

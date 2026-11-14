//! Only safe fixture metadata is written here. The parent owns log collection.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub struct Capture {
    path: PathBuf,
    metadata: Value,
}
impl Capture {
    pub fn new(dir: &Path, pid: u32, env: &[(&str, &str)]) -> Option<Self> {
        std::env::var_os("CMAEG_HOST_EVIDENCE")?;
        let value = |key| {
            env.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| *value)
        };
        let replicas: usize = value("CMAEG_HOST_REPLICAS")?.parse().unwrap();
        let ordinal: usize = value("CMAEG_HOST_ORDINAL")?.parse().unwrap();
        let enabled: bool = value("CMAEG_REPLICAS_ENABLED")?.parse().unwrap();
        let executable =
            std::env::var("CMAEG_HOST_APPLICATION_SHA256").expect("bound executable hash");
        let identity = format!(
            "{:x}",
            Sha256::digest(format!("{}:{pid}:{executable}", uuid::Uuid::new_v4()))
        );
        let capture = Self {
            path: dir.join("capture.json"),
            metadata: json!({
                "replicas": replicas, "enabled": enabled, "ordinal": ordinal,
                "process_sha256": identity, "executable_sha256": executable, "joined": false,
            }),
        };
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&capture.path)
            .expect("process metadata file");
        serde_json::to_writer(file, &capture.metadata).expect("process metadata write");
        Some(capture)
    }
    pub fn joined(&mut self) -> Result<(), &'static str> {
        self.metadata["joined"] = true.into();
        let pending = self.path.with_extension("pending");
        let file = std::fs::File::create(&pending).map_err(|_| "metadata_create")?;
        serde_json::to_writer(file, &self.metadata).map_err(|_| "metadata_write")?;
        std::fs::rename(pending, &self.path).map_err(|_| "metadata_rename")
    }
}

/// Required by the selected diagnostic; append before each fallible boundary.
pub struct Ledger(std::sync::Mutex<std::fs::File>);
impl Ledger {
    pub fn new(replicas: usize, enabled: bool) -> Self {
        let root = std::env::var_os("CMAEG_HOST_EVIDENCE").expect("host evidence root required");
        let path = PathBuf::from(root).join(format!("n{replicas}-{enabled}-ledger.jsonl"));
        Self(std::sync::Mutex::new(
            std::fs::OpenOptions::new()
                .append(true)
                .create_new(true)
                .open(path)
                .expect("host ledger file"),
        ))
    }
    pub fn record(&self, value: Value) {
        let mut file = self.0.lock().unwrap();
        serde_json::to_writer(&mut *file, &value).expect("host ledger record");
        writeln!(file).expect("host ledger newline");
        file.sync_data().expect("host ledger durable write");
    }
}

pub fn verify_application() {
    if std::env::var_os("CMAEG_HOST_EVIDENCE").is_some() {
        let path = std::env::var_os("CMAEG_HOST_APPLICATION").expect("bound application required");
        assert_eq!(
            Path::new(&path),
            Path::new(env!("CARGO_BIN_EXE_cmaeg")),
            "compiled application path"
        );
    }
}

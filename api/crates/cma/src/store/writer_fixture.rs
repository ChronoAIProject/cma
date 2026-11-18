//! Shared boundaries for disposable cross-version writer processes.
use mongodb::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) fn input(name: &str) -> String {
    std::env::var(name).expect("writer fixture input is required")
}

pub(crate) fn database_name() -> String {
    let name = input("CMA_WRITER_DATABASE");
    assert!(name.starts_with("cma_820_writer_") || name.starts_with("cma_writer_"));
    assert!(name.len() <= 63 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
    name
}

pub(crate) async fn client() -> Client {
    let mut options = mongodb::options::ClientOptions::parse(input("CHRONO_TEST_MONGODB_URI"))
        .await
        .expect("parse disposable fixture endpoint");
    // Each local image receives this one private Docker-network alias. No
    // caller-supplied remote database may be used by the cross-version fixture.
    assert_eq!(options.hosts.len(), 1);
    assert_eq!(options.hosts[0].to_string(), "mongo:27017");
    options.server_selection_timeout = Some(std::time::Duration::from_secs(5));
    options.connect_timeout = Some(std::time::Duration::from_secs(5));
    Client::with_options(options).expect("configure disposable fixture client")
}

pub(crate) fn hash<T: serde::Serialize>(value: &T) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).unwrap()))
}

pub(crate) fn emit(value: Value) {
    println!("WRITER_RESULT {}", value);
}

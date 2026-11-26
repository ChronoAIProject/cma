use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{channel::Source, seal::seal_blob};

fn keys() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([41; 32]))).unwrap()
}

fn github() -> Source {
    Source::Github {
        app_ref: "pilot".into(),
        installation_ids: vec![7],
        repository_ids: vec![42],
        deny_repository_ids: Vec::new(),
        events: vec!["issues".into()],
        actions: vec!["opened".into()],
        prompt_template: None,
    }
}

fn sealed(github_source: bool) -> Vec<u8> {
    let record = crate::console::test_support::record(1);
    let mut definition = crate::console::test_support::definition(&record, "owner-1", "import");
    if github_source {
        definition.sources.push(github());
    }
    seal_blob(&keys(), definition.to_blob_json().to_string().as_bytes()).unwrap()
}

fn refused(result: Result<(), RecordError>) -> String {
    match result {
        Err(RecordError::Rejected(reason)) => reason,
        other => panic!("expected a rejection, got {other:?}"),
    }
}

#[test]
fn disabled_gate_refuses_a_new_github_source_and_admits_other_channels() {
    let gate = ImportGate::new(false, Some(keys()));
    assert_eq!(
        refused(gate.check_new_channel("", 1, &sealed(true))),
        GITHUB_IMPORT_DISABLED
    );
    gate.check_new_channel("", 1, &sealed(false)).unwrap();
}

#[test]
fn enabled_gate_admits_github_sources_without_opening_the_row() {
    let gate = ImportGate::new(true, None);
    gate.check_new_channel("", 1, &sealed(true)).unwrap();
    gate.check_new_channel("", 1, b"opaque").unwrap();
}

#[test]
fn disabled_gate_fails_closed_without_keys_or_with_an_unreadable_row() {
    assert_eq!(
        refused(ImportGate::new(false, None).check_new_channel("", 1, &sealed(false))),
        GITHUB_IMPORT_KEYS_REQUIRED
    );
    let gate = ImportGate::new(false, Some(keys()));
    assert_eq!(
        refused(gate.check_new_channel("", 1, b"opaque")),
        GITHUB_IMPORT_UNREADABLE
    );
    let foreign = MasterKeyRing::parse(&format!("2:{}", STANDARD.encode([9; 32]))).unwrap();
    assert_eq!(
        refused(ImportGate::new(false, Some(foreign)).check_new_channel("", 1, &sealed(true))),
        GITHUB_IMPORT_UNREADABLE
    );
    let plain = seal_blob(&keys(), b"not a channel definition").unwrap();
    assert_eq!(
        refused(gate.check_new_channel("", 1, &plain)),
        GITHUB_IMPORT_UNREADABLE
    );
}

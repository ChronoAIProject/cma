//! Exercises the production sealed-record codec in separate harness processes.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::seal::{open_blob, seal_blob, MasterKeyRing};
use ring::rand::{SecureRandom, SystemRandom};

#[test]
fn restore_evidence_sealed() {
    let key_spec = std::env::var("HOME_BACKUP_EVIDENCE_KEYS").unwrap_or_else(|_| {
        (0..3)
            .map(|_| {
                let mut bytes = [0; 32];
                SystemRandom::new().fill(&mut bytes).unwrap();
                STANDARD.encode(bytes)
            })
            .collect::<Vec<_>>()
            .join(",")
    });
    let keys: Vec<_> = key_spec.split(',').collect();
    assert!(keys.len() == 3, "synthetic key count");
    let sentinel = std::env::var("HOME_BACKUP_EVIDENCE_SENTINEL")
        .unwrap_or_else(|_| "synthetic-sealed-custody-payload".into());
    let phase = std::env::var("HOME_BACKUP_EVIDENCE_PHASE").unwrap_or_else(|_| "both".into());
    assert!(matches!(phase.as_str(), "write" | "read" | "both"));
    let directory = std::env::var_os("HOME_BACKUP_EVIDENCE_ARTIFACTS");
    for index in 0..2 {
        let spec = format!("{}:{}", index + 1, keys[index]);
        let writer = MasterKeyRing::parse(&spec).unwrap();
        let path = directory
            .as_ref()
            .map(|root| std::path::Path::new(root).join(format!("sealed-{index}.bin")));
        let bytes = if phase == "read" {
            std::fs::read(path.as_ref().expect("artifact directory")).unwrap()
        } else {
            let bytes = seal_blob(&writer, sentinel.as_bytes()).unwrap();
            if let Some(path) = &path {
                std::fs::write(path, &bytes).unwrap();
            }
            bytes
        };
        drop(writer);
        if phase == "write" {
            continue;
        }
        // Reconstituted keyring keeps old ciphertext readable during overlap.
        let restored = MasterKeyRing::parse(&format!("2:{},1:{}", keys[1], keys[0])).unwrap();
        let opened = open_blob(&restored, &bytes).unwrap();
        assert!(
            opened.payload == sentinel.as_bytes(),
            "sealed payload mismatch"
        );
        assert!(opened.needs_rekey == (index == 0), "old key rekey notice");
        let wrong = MasterKeyRing::parse(&format!("{}:{}", index + 1, keys[2])).unwrap();
        let Err(error) = open_blob(&wrong, &bytes) else {
            panic!("wrong key must refuse");
        };
        let new_only = MasterKeyRing::parse(&format!("2:{}", keys[1])).unwrap();
        if index == 0 {
            assert!(
                open_blob(&new_only, &bytes).is_err(),
                "missing old key must refuse"
            );
        }
        println!("sealed custody version {index}: {restored:?}; {error}");
    }
}

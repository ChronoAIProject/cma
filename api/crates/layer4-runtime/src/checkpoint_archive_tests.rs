use super::*;

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    store: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "cma-archive-{}-{name}-{}",
            std::process::id(),
            stamp()
        ));
        let home = root.join("home");
        let store = root.join("store");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&store).unwrap();
        Self { root, home, store }
    }

    fn inspect(&self, bytes: &[u8]) -> Result<InspectedArchive> {
        let path = self.store.join("candidate.cma");
        std::fs::write(&path, bytes).unwrap();
        inspect(&path, &hex(&Sha256::digest(bytes)), &self.home)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn legacy_bytes(manifest: &Manifest, payload: &[u8]) -> Vec<u8> {
    let manifest = serde_json::to_vec(manifest).unwrap();
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&(manifest.len() as u64).to_be_bytes());
    bytes.extend_from_slice(&manifest);
    bytes.extend_from_slice(payload);
    bytes
}

fn compressed(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn single_file(size: u64, sha256: String) -> Manifest {
    Manifest {
        version: crate::checkpoint_manifest::FORMAT_VERSION,
        entries: vec![ManifestEntry {
            path: "file".into(),
            kind: EntryKind::File,
            mode: 0o600,
            size,
            sha256: Some(sha256),
            target: None,
            offset: Some(0),
        }],
    }
}

#[test]
fn captures_are_compressed_deterministic_and_replay_the_exact_object() {
    let fixture = Fixture::new("compressed-capture");
    let bytes = vec![0x35; 1024 * 1024];
    std::fs::write(fixture.home.join("file"), &bytes).unwrap();
    let first = seal(&fixture.home, &fixture.store, &[]).unwrap();
    let replay = seal(&fixture.home, &fixture.store, &[]).unwrap();
    assert_eq!(first.reference, replay.reference);
    assert_eq!(first.sha256, replay.sha256);
    assert_eq!(first.path, replay.path);
    assert_eq!(std::fs::read_dir(&fixture.store).unwrap().count(), 1);
    let encoded = std::fs::read(&first.path).unwrap();
    assert_eq!(&encoded[..4], &[0x1f, 0x8b, 8, 0]);
    assert!(encoded.len() < bytes.len() / 100);
    assert_eq!(first.sha256, hex(&Sha256::digest(&encoded)));
    inspect(&first.path, &first.sha256, &fixture.home).unwrap();
    let restored = fixture.root.join("restored");
    extract(&first.path, &first.sha256, &fixture.home, &restored).unwrap();
    assert_eq!(std::fs::read(restored.join("file")).unwrap(), bytes);
}

#[test]
fn sealed_digest_covers_multiple_buffers_and_the_final_compression_trailer() {
    let fixture = Fixture::new("buffered-capture");
    let mut state = 7_u64;
    let bytes = (0..ARCHIVE_BUFFER_BYTES * 5 + 113)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect::<Vec<_>>();
    std::fs::write(fixture.home.join("binary"), &bytes).unwrap();
    std::fs::write(fixture.home.join("empty"), []).unwrap();
    let sealed = seal(&fixture.home, &fixture.store, &[]).unwrap();
    let encoded = std::fs::read(&sealed.path).unwrap();
    assert!(encoded.len() > ARCHIVE_BUFFER_BYTES * 4);
    assert_eq!(sealed.sha256, hex(&Sha256::digest(&encoded)));
    assert_eq!(sealed.reference, format!("home-{}.cma", sealed.sha256));
    let restored = fixture.root.join("restored");
    extract(&sealed.path, &sealed.sha256, &fixture.home, &restored).unwrap();
    assert_eq!(std::fs::read(restored.join("binary")).unwrap(), bytes);
    assert_eq!(std::fs::metadata(restored.join("empty")).unwrap().len(), 0);
}

#[test]
fn legacy_uncompressed_archives_still_restore_with_exact_manifest_offsets() {
    let fixture = Fixture::new("legacy-restore");
    std::fs::write(fixture.home.join("a"), b"first").unwrap();
    std::fs::write(fixture.home.join("b"), b"second").unwrap();
    let manifest = crate::checkpoint_manifest::collect(&fixture.home, &[]).unwrap();
    let bytes = legacy_bytes(&manifest, b"firstsecond");
    fixture.inspect(&bytes).unwrap();
    let destination = fixture.root.join("restored");
    extract(
        &fixture.store.join("candidate.cma"),
        &hex(&Sha256::digest(&bytes)),
        &fixture.home,
        &destination,
    )
    .unwrap();
    assert_eq!(std::fs::read(destination.join("a")).unwrap(), b"first");
    assert_eq!(std::fs::read(destination.join("b")).unwrap(), b"second");
}

#[test]
fn compressed_corruption_truncation_optional_headers_and_extra_members_are_rejected() {
    let fixture = Fixture::new("compressed-malformed");
    let manifest = single_file(7, hex(&Sha256::digest(b"payload")));
    let valid = compressed(&legacy_bytes(&manifest, b"payload"));
    fixture.inspect(&valid).unwrap();
    let mut corrupt_crc = valid.clone();
    let crc = corrupt_crc.len() - 8;
    corrupt_crc[crc] ^= 1;
    let mut corrupt_stream = valid.clone();
    corrupt_stream[12] ^= 0x80;
    let mut optional_header = valid.clone();
    optional_header[3] = 8;
    let mut trailing = valid.clone();
    trailing.extend_from_slice(b"trailing");
    let mut concatenated = valid.clone();
    concatenated.extend_from_slice(&valid);
    for bytes in [
        valid[..7].to_vec(),
        valid[..valid.len() - 1].to_vec(),
        corrupt_crc,
        corrupt_stream,
        optional_header,
        trailing,
        concatenated,
    ] {
        assert!(fixture.inspect(&bytes).is_err());
    }
    assert_eq!(std::fs::read_dir(&fixture.home).unwrap().count(), 0);
}

#[test]
fn compressed_manifest_and_expansion_remain_bounded_before_extraction() {
    let fixture = Fixture::new("compressed-bounds");
    let mut oversized_manifest = MAGIC.to_vec();
    oversized_manifest.extend_from_slice(&(MAX_MANIFEST_BYTES + 1).to_be_bytes());
    assert!(fixture
        .inspect(&compressed(&oversized_manifest))
        .unwrap_err()
        .detail
        .contains("manifest length"));
    let oversized_payload = single_file(
        crate::checkpoint_manifest::MAX_PAYLOAD_BYTES + 1,
        "0".repeat(64),
    );
    assert!(fixture
        .inspect(&compressed(&legacy_bytes(&oversized_payload, &[])))
        .unwrap_err()
        .detail
        .contains("payload exceeds"));
    let mut forged = legacy_bytes(&single_file(1, hex(&Sha256::digest([0]))), &[0]);
    forged.resize(forged.len() + 1024 * 1024, 0);
    assert!(fixture
        .inspect(&compressed(&forged))
        .unwrap_err()
        .detail
        .contains("payload length"));
    let truncated = legacy_bytes(&single_file(2, "0".repeat(64)), &[0]);
    assert!(fixture
        .inspect(&compressed(&truncated))
        .unwrap_err()
        .detail
        .contains("payload length"));
}

#[test]
fn compressed_manifests_cannot_bypass_path_link_or_offset_validation() {
    let fixture = Fixture::new("compressed-paths");
    let base = single_file(1, hex(&Sha256::digest([0])));
    let mut escaping_path = base.clone();
    escaping_path.entries[0].path = "../outside".into();
    let mut escaping_link = base.clone();
    escaping_link.entries[0] = ManifestEntry {
        path: "link".into(),
        kind: EntryKind::Symlink,
        mode: 0o777,
        size: 0,
        sha256: None,
        target: Some("../../outside".into()),
        offset: None,
    };
    let mut wrong_offset = base;
    wrong_offset.entries[0].offset = Some(1);
    for manifest in [escaping_path, escaping_link, wrong_offset] {
        assert!(fixture
            .inspect(&compressed(&legacy_bytes(&manifest, &[0])))
            .is_err());
    }
    assert!(!fixture.root.join("outside").exists());
}

#[tokio::test]
async fn corrupt_inner_file_never_replaces_home_and_upload_replays_remain_exact() {
    let fixture = Fixture::new("restore-integrity");
    std::fs::write(fixture.home.join("preserved"), b"owner data").unwrap();
    let mut config = crate::config::RuntimeConfig::for_test(fixture.home.clone());
    config.checkpoint_root = fixture.store.clone();
    let store = crate::checkpoint::CheckpointStore::new(&config);
    let manifest = single_file(7, hex(&Sha256::digest(b"payload")));
    let bytes = compressed(&legacy_bytes(&manifest, b"changed"));
    let digest = hex(&Sha256::digest(&bytes));
    let reference = format!("home-{digest}.cma");
    for _ in 0..2 {
        let (temporary, expected) = store.upload_path(&reference).unwrap();
        std::fs::write(&temporary, &bytes).unwrap();
        store
            .commit_upload(temporary, reference.clone(), expected)
            .await
            .unwrap();
    }
    assert!(store
        .restore(&layer3_adapter::RestoreRequest {
            object_reference: reference,
            sha256: digest
        })
        .await
        .is_err());
    assert_eq!(
        std::fs::read(fixture.home.join("preserved")).unwrap(),
        b"owner data"
    );
    assert!(!fixture.home.join("file").exists());
    assert_eq!(std::fs::read_dir(&fixture.store).unwrap().count(), 1);
}

#[tokio::test]
async fn restore_rejects_late_corruption_without_replacing_home_or_leaving_staging() {
    let fixture = Fixture::new("restore-late-corruption");
    std::fs::write(fixture.home.join("preserved"), b"owner data").unwrap();
    let mut config = crate::config::RuntimeConfig::for_test(fixture.home.clone());
    config.checkpoint_root = fixture.store.clone();
    let store = crate::checkpoint::CheckpointStore::new(&config);
    let manifest = single_file(7, hex(&Sha256::digest(b"payload")));
    let valid = compressed(&legacy_bytes(&manifest, b"payload"));
    let mut corrupt_crc = valid.clone();
    let crc = corrupt_crc.len() - 8;
    corrupt_crc[crc] ^= 1;
    let mut trailing = valid.clone();
    trailing.extend_from_slice(b"trailing");
    let mut concatenated = valid.clone();
    concatenated.extend_from_slice(&valid);
    for bytes in [
        corrupt_crc,
        valid[..valid.len() - 1].to_vec(),
        trailing,
        concatenated,
        compressed(&legacy_bytes(&manifest, b"payloadextra")),
        compressed(&legacy_bytes(&manifest, b"short")),
    ] {
        let digest = hex(&Sha256::digest(&bytes));
        let reference = format!("home-{digest}.cma");
        let path = fixture.store.join(&reference);
        std::fs::write(&path, &bytes).unwrap();
        assert!(store
            .restore(&layer3_adapter::RestoreRequest {
                object_reference: reference,
                sha256: digest,
            })
            .await
            .is_err());
        assert_eq!(
            std::fs::read(fixture.home.join("preserved")).unwrap(),
            b"owner data"
        );
        assert_eq!(std::fs::read_dir(&fixture.home).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(&fixture.store).unwrap().count(), 1);
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn compressed_output_writer_and_stored_input_keep_the_physical_archive_bound() {
    let fixture = Fixture::new("physical-bound");
    let path = fixture.store.join("bounded");
    let file = std::fs::File::create(&path).unwrap();
    let mut writer = ArchiveWriter {
        file: BufWriter::new(file),
        size: MAX_ARCHIVE_BYTES - 1,
        digest: Sha256::new(),
    };
    assert!(writer.write_all(&[0, 1]).is_err());
    assert_eq!(writer.size, MAX_ARCHIVE_BYTES - 1);
    assert_eq!(writer.digest.clone().finalize(), Sha256::digest([]));
    writer
        .file
        .get_ref()
        .set_len(MAX_ARCHIVE_BYTES + 1)
        .unwrap();
    assert!(inspect(&path, &"0".repeat(64), &fixture.home)
        .unwrap_err()
        .detail
        .contains("size limit"));
}

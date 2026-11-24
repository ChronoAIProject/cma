use super::*;
use layer3_adapter::{
    archive_source::{Kind, SnapshotTree},
    permanent_custody::{digest, CustodyPolicy, ObjectReceipt, PermanentCustody},
};
use std::{
    os::unix::fs::{DirBuilderExt, FileTypeExt},
    path::PathBuf,
    sync::Mutex,
};

struct ReadFault {
    inner: Custody,
    fault: Mutex<Option<(String, PathBuf, bool)>>,
}

#[async_trait::async_trait]
impl PermanentCustody for ReadFault {
    async fn policy(&self) -> Result<CustodyPolicy> {
        self.inner.policy().await
    }
    async fn retain(&self, operation: &str, bytes: Vec<u8>) -> Result<ObjectReceipt> {
        self.inner.retain(operation, bytes).await
    }
    async fn read(&self, receipt: &ObjectReceipt) -> Result<Vec<u8>> {
        let bytes = self.inner.read(receipt).await?;
        let mut fault = self.fault.lock().unwrap();
        if fault
            .as_ref()
            .is_some_and(|(sha, _, _)| sha == &receipt.sha256)
        {
            let (_, path, remove) = fault.take().unwrap();
            if remove {
                std::fs::remove_file(path).unwrap();
            } else {
                std::fs::write(path, b"existing destination").unwrap();
            }
        }
        Ok(bytes)
    }
}

struct Fixture {
    temp: Temp,
    request: Request,
    bytes: Vec<u8>,
    custody: ReadFault,
    receipt: ObjectReceipt,
    manifest: Manifest,
    source: Vec<SnapshotTree>,
}

async fn fixture() -> Fixture {
    let path = std::fs::canonicalize("/tmp")
        .unwrap()
        .join(format!("archive-{}", uuid::Uuid::new_v4().simple()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    let temp = Temp(path);
    let mut request = request(&temp);
    let socket = temp.0.join("s");
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    std::fs::rename(socket, request.roots[0].path.join("s".repeat(120))).unwrap();
    std::fs::write(request.roots[0].path.join("z"), b"after socket").unwrap();
    request.source_sha256 = digest(b"original source");
    request.exclusion_sha256 = digest(b"original exclusion");
    let bytes = serde_json::to_vec(&request).unwrap();
    let custody = ReadFault {
        inner: Custody::default(),
        fault: Mutex::new(None),
    };
    let mut files = Vec::new();
    let mut source = Vec::new();
    for root in &request.roots {
        let tree = SnapshotTree::open(&root.path).unwrap();
        for entry in &tree.entries {
            let mut data = Vec::new();
            if entry.kind == Kind::File {
                let mut reader = tree.reader(entry).unwrap();
                while let Some(bytes) = reader.chunk().unwrap() {
                    data.extend(bytes);
                }
            }
            let objects = if entry.kind == Kind::File {
                vec![custody.retain(OP, data.clone()).await.unwrap()]
            } else {
                vec![]
            };
            files.push(FileRecord {
                role: root.role.clone(),
                entry: entry.clone(),
                sha256: digest(&data),
                objects,
            });
        }
        source.push(tree);
    }
    let manifest = Manifest {
        format: 1,
        operation_id: request.operation_id.clone(),
        request_sha256: digest(&bytes),
        original_request: custody.retain(OP, bytes.clone()).await.unwrap(),
        original_source: custody
            .retain(OP, b"original source".to_vec())
            .await
            .unwrap(),
        exclusion: custody
            .retain(OP, b"original exclusion".to_vec())
            .await
            .unwrap(),
        custody: custody.policy().await.unwrap(),
        schema_sha256: crate::store::retirement_archive::schema_sha256(),
        files,
        database: crate::store::retirement_archive::manifest_collections()
            .iter()
            .map(|name| DatabaseRecord {
                collection: (*name).into(),
                documents: 0,
                bytes: 0,
                sha256: digest(&[]),
                objects: vec![],
            })
            .collect(),
        external: vec![],
    };
    let receipt = custody
        .retain(OP, serde_json::to_vec(&manifest).unwrap())
        .await
        .unwrap();
    Fixture {
        temp,
        request,
        bytes,
        custody,
        receipt,
        manifest,
        source,
    }
}

#[tokio::test]
async fn socket_restore_verification_retains_the_original_manifest_and_complete_census() {
    let fixture = fixture().await;
    let destination = fixture.temp.0.join("destination".repeat(16));
    let verified = verify::restore(
        &fixture.custody,
        &fixture.receipt,
        &fixture.request,
        &fixture.bytes,
        &destination,
    )
    .await
    .unwrap();
    assert_eq!(verified.files, fixture.manifest.files.len() as u64);
    assert!(
        std::fs::symlink_metadata(destination.join("home").join("s".repeat(120)))
            .unwrap()
            .file_type()
            .is_socket()
    );
    assert_eq!(
        std::fs::read(destination.join("manifest.json")).unwrap(),
        serde_json::to_vec(&fixture.manifest).unwrap()
    );
    for tree in fixture.source {
        tree.verify_unchanged().unwrap();
    }
}

#[tokio::test]
async fn socket_placement_and_final_census_failures_never_return_verification() {
    for remove in [false, true] {
        let fixture = fixture().await;
        let destination = fixture.temp.0.join("restore");
        let socket = destination.join("home").join("s".repeat(120));
        let trigger = if remove {
            b"after socket".as_slice()
        } else {
            b"home bytes".as_slice()
        };
        *fixture.custody.fault.lock().unwrap() = Some((digest(trigger), socket.clone(), remove));
        let before = fixture.custody.inner.0.lock().unwrap().clone();
        assert!(verify::restore(
            &fixture.custody,
            &fixture.receipt,
            &fixture.request,
            &fixture.bytes,
            &destination
        )
        .await
        .is_err());
        assert!(
            fixture.custody.fault.lock().unwrap().is_none(),
            "fault must reach the real reconstruction"
        );
        assert!(!destination.join("manifest.json").exists());
        assert_eq!(
            *fixture.custody.inner.0.lock().unwrap(),
            before,
            "restore cannot alter the retained archive or publish a proof on failure"
        );
        if !remove {
            assert_eq!(std::fs::read(socket).unwrap(), b"existing destination");
        }
        assert!(verify::restore(
            &fixture.custody,
            &fixture.receipt,
            &fixture.request,
            &fixture.bytes,
            &destination
        )
        .await
        .is_err());
        verify::restore(
            &fixture.custody,
            &fixture.receipt,
            &fixture.request,
            &fixture.bytes,
            &fixture.temp.0.join("fresh-retry"),
        )
        .await
        .unwrap();
        for tree in fixture.source {
            tree.verify_unchanged().unwrap();
        }
    }
}

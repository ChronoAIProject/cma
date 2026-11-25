use super::*;
use layer3_adapter::permanent_custody::{digest, CustodyPolicy, ObjectReceipt, PermanentCustody};
use std::{collections::BTreeMap, sync::Mutex};
#[path = "socket_tests.rs"]
mod sockets;
pub(crate) const OP: &str = "de98646f-1234-4234-8234-0123456789ab";
#[derive(Default)]
pub(crate) struct Custody(pub Mutex<BTreeMap<String, Vec<u8>>>);
#[async_trait::async_trait]
impl PermanentCustody for Custody {
    async fn policy(&self) -> Result<CustodyPolicy> {
        Ok(CustodyPolicy {
            format: 1,
            bucket: "fixture".into(),
            metageneration: "1".into(),
            permanent_hold: true,
            private_uniform_access: true,
            independent_read_only: true,
        })
    }
    async fn retain(&self, operation: &str, bytes: Vec<u8>) -> Result<ObjectReceipt> {
        let sha = digest(&bytes);
        let key = format!("retirement-v1/{operation}/{sha}");
        let size = bytes.len() as u64;
        let mut records = self.0.lock().unwrap();
        if let Some(old) = records.get(&key) {
            if old != &bytes {
                return Err(REFUSED);
            }
        } else {
            records.insert(key.clone(), bytes);
        }
        Ok(ObjectReceipt {
            key,
            generation: "1".into(),
            sha256: sha,
            bytes: size,
        })
    }
    async fn read(&self, receipt: &ObjectReceipt) -> Result<Vec<u8>> {
        let bytes = self
            .0
            .lock()
            .unwrap()
            .get(&receipt.key)
            .cloned()
            .ok_or(REFUSED)?;
        if digest(&bytes) != receipt.sha256 || bytes.len() as u64 != receipt.bytes {
            return Err(REFUSED);
        }
        Ok(bytes)
    }
}
pub(crate) struct Temp(pub std::path::PathBuf);
impl Temp {
    pub(crate) fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let path = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("cma-retirement-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    pub(crate) fn write(&self, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
pub(crate) fn request(temp: &Temp) -> Request {
    use std::os::unix::fs::MetadataExt;
    let mut roots = Vec::new();
    for role in ["home", "state", "transfer"] {
        let path = temp.0.join(role);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("data"), format!("{role} bytes")).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        roots.push(Root {
            role: role.into(),
            path,
            device: meta.dev(),
            inode: meta.ino(),
        });
    }
    Request {
        format: 1,
        operation_id: OP.into(),
        sandbox_id: "sbx_archive".into(),
        expected_revision: 1,
        source_sha256: "a".repeat(64),
        retirement_intent_sha256: "b".repeat(64),
        home_identity: "home-identity".into(),
        cohort_sha256: "c".repeat(64),
        epoch: 1,
        roots,
        exclusion_receipt: temp.0.join("exclusion.json"),
        exclusion_sha256: "d".repeat(64),
    }
}
pub(crate) fn qualify(temp: &Temp, request: &mut Request) {
    let now = layer2_core::now_rfc3339();
    let now = layer2_core::seconds_from_rfc3339(&now).unwrap();
    let exclusion = ExclusionReceipt {
        format: 1,
        operation_id: request.operation_id.clone(),
        sandbox_id: request.sandbox_id.clone(),
        source_sha256: request.source_sha256.clone(),
        cohort_sha256: request.cohort_sha256.clone(),
        home_identity: request.home_identity.clone(),
        roots_sha256: request.roots_sha256().unwrap(),
        epoch: request.epoch,
        qualified_at: layer2_core::rfc3339_at(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(now),
        ),
        expires_at: layer2_core::rfc3339_at(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(now + 3600),
        ),
        writer_matrix_sha256: "e".repeat(64),
        provider_finality_sha256: "f".repeat(64),
        physical_fence_sha256: "a".repeat(64),
        retirement_intent_sha256: request.retirement_intent_sha256.clone(),
    };
    let data = serde_json::to_vec(&exclusion).unwrap();
    request.exclusion_receipt = temp.write("exclusion.json", &data);
    request.exclusion_sha256 = digest(&data);
}
#[test]
fn exclusion_refuses_expired_wrong_source_epoch_and_root_authority() {
    let temp = Temp::new();
    let mut request = request(&temp);
    qualify(&temp, &mut request);
    export::exclusion(&request).unwrap();
    for case in 0..4 {
        let mut bad = request.clone();
        match case {
            0 => bad.epoch += 1,
            1 => bad.source_sha256 = "f".repeat(64),
            2 => bad.roots[0].inode += 1,
            _ => bad.exclusion_sha256 = "e".repeat(64),
        }
        assert!(export::exclusion(&bad).is_err());
    }
    let mut value: ExclusionReceipt =
        serde_json::from_slice(&std::fs::read(&request.exclusion_receipt).unwrap()).unwrap();
    value.expires_at = value.qualified_at.clone();
    assert!(value
        .require(&request, &layer2_core::now_rfc3339())
        .is_err());
}
#[test]
fn bson_restore_refuses_truncation_duplicate_identity_and_invalid_frame() {
    use mongodb::bson::{doc, serialize_to_vec};
    let bytes = serialize_to_vec(&doc! {"_id":"fixture","conversation":"complete body"}).unwrap();
    assert_eq!(
        verify::validate_bson(&bytes, &mut Default::default()).unwrap(),
        1
    );
    let mut duplicate = bytes.clone();
    duplicate.extend(&bytes);
    assert!(verify::validate_bson(&duplicate, &mut Default::default()).is_err());
    assert!(verify::validate_bson(&bytes[..bytes.len() - 1], &mut Default::default()).is_err());
    assert!(verify::validate_bson(&[0; 4], &mut Default::default()).is_err());
}
#[tokio::test]
async fn disabled_command_refuses_before_database_or_source_access() {
    let temp = Temp::new();
    let invocation = temp.write(
        "command.json",
        br#"{"request_file":"/missing","output_file":"/missing"}"#,
    );
    let args = vec![
        "--retirement-archive".into(),
        "complete".into(),
        invocation.to_string_lossy().into(),
    ];
    assert_eq!(command(&args).await, Err(REFUSED));
}

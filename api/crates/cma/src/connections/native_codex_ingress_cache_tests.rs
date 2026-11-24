use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use layer3_adapter::ingress_budget::{ClassBounds, IngressBudget};
use std::{
    fs,
    io::{self, Cursor, Seek, SeekFrom, Write},
    path::PathBuf,
};

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("synthetic native cache fixture", &[bytes]).unwrap()
}

fn cache() -> SecretValue {
    let claims = serde_json::json!({"exp": 1000, "https://api.openai.com/auth": {
        "chatgpt_account_id": "fixture", "chatgpt_plan_type": "plus"
    }});
    let token = format!(
        "fixture.{}.fixture",
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    SecretValue::new(
        serde_json::json!({"tokens": {
            "access_token": token, "refresh_token": "synthetic-refresh", "id_token": "synthetic-id",
            "account_id": "fixture"
        }})
        .to_string(),
    )
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(32 * 1024 * 1024);
    let bounds = ClassBounds::new(root.capacity_bytes(), plan(4096), plan(4096)).unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    (root, policy)
}

fn own(policy: &IngressOwnershipPolicy) -> SharedOwned<SecretValue> {
    let input = cache();
    policy
        .admit(
            IngressClass::Data,
            plan(sizing::shared_owner_bytes::<SecretValue>() + input.expose().len()),
        )
        .unwrap()
        .initialize(|| input)
}

fn reader(policy: &IngressOwnershipPolicy) -> CacheRead {
    CacheRead::admit(&own(policy), policy).unwrap()
}

fn padded(bytes: usize) -> Vec<u8> {
    let mut bytes_value = cache().expose().as_bytes().to_vec();
    assert!(bytes_value.len() <= bytes);
    bytes_value.resize(bytes, b' ');
    bytes_value
}

struct FileFixture(PathBuf);

impl FileFixture {
    fn new(bytes: &[u8]) -> Self {
        use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
        let home =
            std::env::temp_dir().join(format!("cma-cache-read-test-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
        let fixture = Self(home);
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(fixture.path())
            .unwrap()
            .write_all(bytes)
            .unwrap();
        fixture
    }

    fn path(&self) -> PathBuf {
        self.0.join("auth.json")
    }
    fn open(&self) -> File {
        File::open(self.path()).unwrap()
    }
}

impl Drop for FileFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn exact_limit_preserves_original_bytes_and_owner() {
    let (root, policy) = policy();
    let baseline = root.available_bytes();
    let bytes = padded(MAX_CACHE_BYTES);
    let result = reader(&policy).read(&mut Cursor::new(&bytes)).unwrap();
    assert_eq!(result.expose().as_bytes(), bytes);
    assert!(result.shares_custody_domain(&policy));
    assert_eq!(
        baseline - root.available_bytes(),
        read_plan().unwrap().requested_bytes()
    );
    drop(result);
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn one_byte_over_is_invalid_account_and_releases_backing() {
    let (root, policy) = policy();
    let baseline = root.available_bytes();
    let error = reader(&policy)
        .read(&mut Cursor::new(padded(READ_BYTES)))
        .unwrap_err();
    assert_eq!(
        error,
        CacheDecodeError::Account(SubscriptionError::InvalidAccount)
    );
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn infinite_source_stops_at_the_detection_byte() {
    struct Infinite {
        read: usize,
    }
    impl Read for Infinite {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            assert!(!buf.is_empty());
            buf.fill(b' ');
            self.read += buf.len();
            Ok(buf.len())
        }
    }
    let (_, policy) = policy();
    let mut source = Infinite { read: 0 };
    assert_eq!(
        reader(&policy).read(&mut source).unwrap_err(),
        CacheDecodeError::Account(SubscriptionError::InvalidAccount)
    );
    assert_eq!(source.read, READ_BYTES);
}

#[test]
fn empty_and_malformed_cache_keep_decoder_classification() {
    let (root, policy) = policy();
    let baseline = root.available_bytes();
    for bytes in [b"".as_slice(), b"{", b"{}"] {
        assert_eq!(
            reader(&policy).read(&mut Cursor::new(bytes)).unwrap_err(),
            CacheDecodeError::Account(SubscriptionError::InvalidAccount)
        );
        assert_eq!(root.available_bytes(), baseline);
    }
}

#[test]
fn invalid_utf8_keeps_legacy_unavailable_classification() {
    let (root, policy) = policy();
    let baseline = root.available_bytes();
    assert_eq!(
        reader(&policy)
            .read(&mut Cursor::new([0xff, 0xfe]))
            .unwrap_err(),
        CacheDecodeError::Account(SubscriptionError::Unavailable)
    );
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn one_byte_reads_and_interrupted_calls_preserve_the_cache() {
    struct Chunks {
        bytes: Cursor<Vec<u8>>,
        interrupt: bool,
    }
    impl Read for Chunks {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.bytes.read(&mut buf[..1])
        }
    }
    let (_, policy) = policy();
    let bytes = padded(1024);
    let mut source = Chunks {
        bytes: Cursor::new(bytes.clone()),
        interrupt: false,
    };
    assert_eq!(
        reader(&policy)
            .read(&mut source)
            .unwrap()
            .expose()
            .as_bytes(),
        bytes
    );
}

#[test]
fn partial_read_failure_frees_the_original_buffer() {
    struct Fails(bool);
    impl Read for Fails {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0 {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            self.0 = true;
            buf[..16].fill(b'x');
            Ok(16)
        }
    }
    let (root, policy) = policy();
    let baseline = root.available_bytes();
    assert_eq!(
        reader(&policy).read(&mut Fails(false)).unwrap_err(),
        CacheDecodeError::Account(SubscriptionError::Unavailable)
    );
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn file_growth_after_metadata_cannot_expand_the_read() {
    let (_, policy) = policy();
    let fixture = FileFixture::new(&padded(1024));
    let mut file = fixture.open();
    assert_eq!(file.metadata().unwrap().len(), 1024);
    fs::OpenOptions::new()
        .append(true)
        .open(fixture.path())
        .unwrap()
        .write_all(&vec![b' '; READ_BYTES * 2])
        .unwrap();
    assert_eq!(
        reader(&policy).read_file(&mut file).unwrap_err(),
        CacheDecodeError::Account(SubscriptionError::InvalidAccount)
    );
    assert_eq!(file.stream_position().unwrap(), READ_BYTES as u64);
}

#[test]
fn path_replacement_does_not_redirect_an_opened_handle() {
    let (_, policy) = policy();
    let original = padded(1024);
    let fixture = FileFixture::new(&original);
    let mut file = fixture.open();
    fs::rename(fixture.path(), fixture.0.join("retained.json")).unwrap();
    fs::write(fixture.path(), b"{}").unwrap();
    let result = reader(&policy).read_file(&mut file).unwrap();
    assert_eq!(result.expose().as_bytes(), original);
}

#[test]
fn non_regular_opened_object_is_rejected() {
    let (_, policy) = policy();
    let fixture = FileFixture::new(b"{}");
    assert_eq!(
        reader(&policy)
            .read_file(&mut File::open(&fixture.0).unwrap())
            .unwrap_err(),
        CacheDecodeError::Account(SubscriptionError::InvalidAccount)
    );
}

#[test]
fn original_read_rewinds_the_descriptor() {
    let (_, policy) = policy();
    let bytes = padded(1024);
    let fixture = FileFixture::new(&bytes);
    let mut file = fixture.open();
    file.seek(SeekFrom::End(0)).unwrap();
    assert_eq!(
        reader(&policy)
            .read_file(&mut file)
            .unwrap()
            .expose()
            .as_bytes(),
        bytes
    );
}

#[test]
fn pressure_refuses_before_read_buffer_construction() {
    let (root, policy) = policy();
    let input = own(&policy);
    let occupied = policy
        .admit(
            IngressClass::Data,
            plan(policy.budget(IngressClass::Data).available_bytes()),
        )
        .unwrap();
    let before = root.available_bytes();
    assert!(matches!(
        CacheRead::admit(&input, &policy),
        Err(CacheDecodeError::Refused(
            AdmissionRefusal::AtCapacity { .. }
        ))
    ));
    assert_eq!(root.available_bytes(), before);
    drop(occupied);
}

#[test]
fn decode_capacity_refusal_does_not_become_sign_in_required() {
    let (root, policy) = policy();
    let read = reader(&policy);
    let occupied = policy
        .admit(
            IngressClass::Data,
            plan(policy.budget(IngressClass::Data).available_bytes()),
        )
        .unwrap();
    let before = root.available_bytes();
    assert!(matches!(
        read.read(&mut Cursor::new(padded(1024))),
        Err(CacheDecodeError::Refused(
            AdmissionRefusal::AtCapacity { .. }
        ))
    ));
    assert_eq!(
        root.available_bytes(),
        before + read_plan().unwrap().requested_bytes()
    );
    drop(occupied);
}

#[test]
fn foreign_root_is_rejected_before_storage_admission() {
    let (root, local) = policy();
    let (foreign_root, foreign) = policy();
    let input = own(&local);
    let before = (root.available_bytes(), foreign_root.available_bytes());
    assert!(matches!(
        CacheRead::admit(&input, &foreign),
        Err(CacheDecodeError::Refused(
            AdmissionRefusal::InvalidPlan { .. }
        ))
    ));
    assert_eq!(
        (root.available_bytes(), foreign_root.available_bytes()),
        before
    );
}

#[test]
fn attachment_directions_and_sibling_are_rejected() {
    let root = IngressBudget::new(128 * 1024 * 1024);
    let bounds = IngressOwnershipPolicy::conversation_bounds(
        root.capacity_bytes(),
        2,
        plan(4096),
        plan(16 * 1024 * 1024),
    )
    .unwrap();
    let parent = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    let first = parent.admit_attachment().unwrap();
    let second = parent.admit_attachment().unwrap();
    let attached = own(first.policy());
    let process = own(&parent);
    let before = root.available_bytes();
    for (input, policy) in [
        (&attached, &parent),
        (&process, first.policy()),
        (&attached, second.policy()),
    ] {
        assert!(matches!(
            CacheRead::admit(input, policy),
            Err(CacheDecodeError::Refused(
                AdmissionRefusal::InvalidPlan { .. }
            ))
        ));
        assert_eq!(root.available_bytes(), before);
    }
}

#[test]
fn returned_cache_keeps_attachment_until_its_last_clone_drops() {
    let root = IngressBudget::new(128 * 1024 * 1024);
    let bounds = IngressOwnershipPolicy::conversation_bounds(
        root.capacity_bytes(),
        1,
        plan(4096),
        plan(16 * 1024 * 1024),
    )
    .unwrap();
    let parent = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    let baseline = root.available_bytes();
    let attachment = parent.admit_attachment().unwrap();
    let input = own(attachment.policy());
    let read = CacheRead::admit(&input, attachment.policy()).unwrap();
    drop(input);
    let result = read.read(&mut Cursor::new(padded(1024))).unwrap();
    assert!(result.shares_custody_domain(attachment.policy()));
    let clone = result.clone();
    drop((result, attachment));
    assert!(parent.admit_attachment().is_err());
    assert_eq!(clone.expose().len(), 1024);
    drop(clone);
    assert_eq!(root.available_bytes(), baseline);
    assert!(parent.admit_attachment().is_ok());
}

use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use layer3_adapter::ingress_budget::{ClassBounds, IngressBudget};
use std::{
    fs,
    os::unix::fs::{symlink, DirBuilderExt as _},
    path::PathBuf,
};

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("synthetic original-open fixture", &[bytes]).unwrap()
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(32 * 1024 * 1024);
    let bounds = ClassBounds::new(root.capacity_bytes(), plan(4096), plan(4096)).unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    (root, policy)
}

fn input(policy: &IngressOwnershipPolicy) -> SharedOwned<SecretValue> {
    policy
        .admit(
            IngressClass::Data,
            plan(sizing::shared_owner_bytes::<SecretValue>()),
        )
        .unwrap()
        .initialize(|| SecretValue::new(String::new()))
}

fn job(home: &SharedOwned<OwnedPrivateHome>) -> NativeHomeJob {
    NativeHomeJob {
        generation: home.identity.generation,
        home: home.clone(),
    }
}

fn prepare(
    home: &SharedOwned<OwnedPrivateHome>,
    input: &SharedOwned<SecretValue>,
    policy: &IngressOwnershipPolicy,
) -> Result<PreparedCacheOpen, CacheDecodeError> {
    PreparedCacheOpen::admit(&job(home), home, input, policy)
}

fn valid() -> Vec<u8> {
    let claims = serde_json::json!({"exp": 1000, "https://api.openai.com/auth": {
        "chatgpt_account_id": "fixture", "chatgpt_plan_type": "plus"
    }});
    let token = format!(
        "fixture.{}.fixture",
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    serde_json::json!({"tokens": {"access_token": token, "refresh_token": "synthetic-refresh",
        "id_token": "synthetic-id", "account_id": "fixture"}})
    .to_string()
    .into_bytes()
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("cma-native-open-test-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("home"))
            .unwrap();
        Self(root)
    }
    fn path(&self) -> PathBuf {
        self.0.join("home/auth.json")
    }
    fn home(&self, policy: &IngressOwnershipPolicy) -> SharedOwned<OwnedPrivateHome> {
        // Fixture-only bootstrap. This is not the production home constructor.
        policy
            .admit(
                IngressClass::Data,
                plan(sizing::shared_owner_bytes::<OwnedPrivateHome>()),
            )
            .unwrap()
            .initialize(|| {
                let directory = File::open(self.0.join("home")).unwrap();
                let metadata = directory.metadata().unwrap();
                OwnedPrivateHome {
                    directory,
                    identity: super::super::HomeIdentity {
                        device: metadata.dev(),
                        inode: metadata.ino(),
                        generation: uuid::Uuid::new_v4().as_u128(),
                    },
                }
            })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn original_open_reads_valid_cache_in_the_exact_domain() {
    let fixture = Fixture::new();
    let (root, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    let baseline = root.available_bytes();
    let bytes = valid();
    fs::write(fixture.path(), &bytes).unwrap();
    let opened = prepare(&home, &input, &policy).unwrap().open().unwrap();
    assert!(opened.shares_custody_domain(&policy));
    let CacheReadCompletion {
        file: opened,
        outcome,
    } = opened.read();
    let cache = outcome.unwrap();
    assert_eq!(cache.expose().as_bytes(), bytes);
    assert!(cache.shares_custody_domain(&policy));
    drop(opened);
    assert_eq!(root.available_bytes(), baseline - cache.requested_bytes());
    drop(cache);
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn renamed_home_and_replacement_path_cannot_redirect_open() {
    let fixture = Fixture::new();
    let (_, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    let bytes = valid();
    fs::write(fixture.path(), &bytes).unwrap();
    fs::rename(fixture.0.join("home"), fixture.0.join("retained-home")).unwrap();
    fs::create_dir(fixture.0.join("home")).unwrap();
    fs::write(fixture.path(), b"{}").unwrap();
    let opened = prepare(&home, &input, &policy).unwrap().open().unwrap();
    assert_eq!(opened.read().outcome.unwrap().expose().as_bytes(), bytes);
}

#[test]
fn replaced_cache_path_cannot_redirect_the_opened_cursor() {
    let fixture = Fixture::new();
    let (_, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    let bytes = valid();
    fs::write(fixture.path(), &bytes).unwrap();
    let opened = prepare(&home, &input, &policy).unwrap().open().unwrap();
    fs::rename(fixture.path(), fixture.0.join("original.json")).unwrap();
    fs::write(fixture.path(), b"{}").unwrap();
    assert_eq!(opened.read().outcome.unwrap().expose().as_bytes(), bytes);
}

#[test]
fn cache_open_after_atomic_replacement_reads_the_new_document() {
    let fixture = Fixture::new();
    let (_, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    fs::write(fixture.path(), valid()).unwrap();
    let mut renewed: serde_json::Value = serde_json::from_slice(&valid()).unwrap();
    renewed["tokens"]["login_owner_id"] = "synthetic-login".into();
    let renewed = serde_json::to_vec(&renewed).unwrap();
    let replacement = fixture.0.join("home/renewed.json");
    fs::write(&replacement, &renewed).unwrap();
    fs::rename(replacement, fixture.path()).unwrap();
    let opened = prepare(&home, &input, &policy).unwrap().open().unwrap();
    assert_eq!(opened.read().outcome.unwrap().expose().as_bytes(), renewed);
}

#[test]
fn symlink_entry_is_invalid_even_when_target_is_valid() {
    let fixture = Fixture::new();
    let (_, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    let target = fixture.0.join("other.json");
    fs::write(&target, valid()).unwrap();
    symlink(target, fixture.path()).unwrap();
    assert!(matches!(
        prepare(&home, &input, &policy).unwrap().open(),
        Err(CacheDecodeError::Account(SubscriptionError::InvalidAccount))
    ));
}

#[test]
fn directory_entry_is_invalid() {
    let fixture = Fixture::new();
    let (_, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    fs::create_dir(fixture.path()).unwrap();
    assert!(matches!(
        prepare(&home, &input, &policy).unwrap().open(),
        Err(CacheDecodeError::Account(SubscriptionError::InvalidAccount))
    ));
}

#[test]
fn fifo_without_writer_is_rejected_without_blocking() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt as _};
    let fixture = Fixture::new();
    let (_, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    let path = CString::new(fixture.path().as_os_str().as_bytes()).unwrap();
    // SAFETY: the owned C string remains valid through this synthetic syscall.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert!(matches!(
        prepare(&home, &input, &policy).unwrap().open(),
        Err(CacheDecodeError::Account(SubscriptionError::InvalidAccount))
    ));
}

#[test]
fn missing_entry_is_invalid_account() {
    let fixture = Fixture::new();
    let (_, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    assert!(matches!(
        prepare(&home, &input, &policy).unwrap().open(),
        Err(CacheDecodeError::Account(SubscriptionError::InvalidAccount))
    ));
}

#[test]
fn opened_descriptor_is_close_on_exec() {
    let fixture = Fixture::new();
    let (_, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    fs::write(fixture.path(), valid()).unwrap();
    let opened = prepare(&home, &input, &policy).unwrap().open().unwrap();
    // SAFETY: the capability retains this descriptor through the flag query.
    let flags = unsafe { libc::fcntl(opened.file.as_raw_fd(), libc::F_GETFD) };
    assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0);
}

#[test]
fn data_pressure_refuses_preparation_before_any_open() {
    let fixture = Fixture::new();
    let (root, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(IngressClass::Data, plan(available))
        .unwrap()
        .initialize(|| vec![1u8; available - sizing::shared_owner_bytes::<Vec<u8>>()]);
    let baseline = root.available_bytes();
    assert!(matches!(
        prepare(&home, &input, &policy),
        Err(CacheDecodeError::Refused(
            AdmissionRefusal::AtCapacity { .. }
        ))
    ));
    assert_eq!(root.available_bytes(), baseline);
    drop(pressure);
}

#[test]
fn foreign_home_or_input_is_rejected_before_preparation() {
    let fixture = Fixture::new();
    let (_, local) = policy();
    let (_, foreign) = policy();
    let local_home = fixture.home(&local);
    let foreign_home = fixture.home(&foreign);
    let local_input = input(&local);
    let foreign_input = input(&foreign);
    for (home, input) in [(&local_home, &foreign_input), (&foreign_home, &local_input)] {
        assert!(matches!(
            prepare(home, input, &local),
            Err(CacheDecodeError::Refused(
                AdmissionRefusal::InvalidPlan { .. }
            ))
        ));
    }
}

#[test]
fn process_attachment_and_sibling_domains_are_not_interchangeable() {
    let fixture = Fixture::new();
    let root = IngressBudget::new(128 * 1024 * 1024);
    let bounds = IngressOwnershipPolicy::conversation_bounds(
        root.capacity_bytes(),
        2,
        plan(4096),
        plan(16 * 1024 * 1024),
    )
    .unwrap();
    let parent = IngressOwnershipPolicy::new(root, bounds).unwrap();
    let first = parent.admit_attachment().unwrap();
    let second = parent.admit_attachment().unwrap();
    let home = fixture.home(first.policy());
    let process_input = input(&parent);
    let input = input(first.policy());
    for wrong in [&parent, second.policy()] {
        assert!(matches!(
            prepare(&home, &input, wrong),
            Err(CacheDecodeError::Refused(
                AdmissionRefusal::InvalidPlan { .. }
            ))
        ));
    }
    assert!(matches!(
        prepare(&home, &process_input, first.policy()),
        Err(CacheDecodeError::Refused(
            AdmissionRefusal::InvalidPlan { .. }
        ))
    ));
    let process_home = fixture.home(&parent);
    assert!(matches!(
        prepare(&process_home, &input, first.policy()),
        Err(CacheDecodeError::Refused(
            AdmissionRefusal::InvalidPlan { .. }
        ))
    ));
}

#[path = "native_codex_ingress_open_lifetime_tests.rs"]
mod lifetime;

#[test]
fn initial_hard_link_is_rejected_even_with_nofollow() {
    let fixture = Fixture::new();
    let (_, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    let outside = fixture.0.join("other.json");
    fs::write(&outside, valid()).unwrap();
    fs::hard_link(&outside, fixture.path()).unwrap();
    assert!(matches!(
        prepare(&home, &input, &policy).unwrap().open(),
        Err(CacheDecodeError::Account(SubscriptionError::InvalidAccount))
    ));
}

#[test]
fn another_jobs_home_is_rejected_within_the_same_policy() {
    let original = Fixture::new();
    let other = Fixture::new();
    let (root, policy) = policy();
    let expected = original.home(&policy);
    let wrong = other.home(&policy);
    let job = job(&expected);
    let input = input(&policy);
    let baseline = root.available_bytes();
    assert!(matches!(
        PreparedCacheOpen::admit(&job, &wrong, &input, &policy),
        Err(CacheDecodeError::Refused(
            AdmissionRefusal::InvalidPlan { .. }
        ))
    ));
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn stale_job_generation_is_rejected_before_preparation() {
    let fixture = Fixture::new();
    let (root, policy) = policy();
    let home = fixture.home(&policy);
    let mut job = job(&home);
    job.generation ^= 1;
    let input = input(&policy);
    let baseline = root.available_bytes();
    assert!(matches!(
        PreparedCacheOpen::admit(&job, &home, &input, &policy),
        Err(CacheDecodeError::Refused(
            AdmissionRefusal::InvalidPlan { .. }
        ))
    ));
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn syscall_errors_are_static_and_do_not_request_sign_in() {
    for (code, expected) in [
        (libc::ENOENT, SubscriptionError::InvalidAccount),
        (libc::ENOTDIR, SubscriptionError::InvalidAccount),
        (libc::ELOOP, SubscriptionError::InvalidAccount),
        (libc::EMFILE, SubscriptionError::Unavailable),
        (libc::EACCES, SubscriptionError::Unavailable),
        (libc::EIO, SubscriptionError::Unavailable),
    ] {
        assert_eq!(
            open_error(std::io::Error::from_raw_os_error(code)),
            CacheDecodeError::Account(expected)
        );
    }
}

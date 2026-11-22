use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

pub(super) fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("synthetic admitted refresh", &[bytes]).unwrap()
}
pub(super) fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(128 * 1024 * 1024);
    let bounds = IngressOwnershipPolicy::conversation_bounds(
        root.capacity_bytes(),
        2,
        plan(4096),
        plan(4096),
    )
    .unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    drop(policy.admit(IngressClass::Data, plan(1)).unwrap());
    (root, policy)
}
pub(super) fn own<T>(
    policy: &IngressOwnershipPolicy,
    create: impl FnOnce() -> T,
) -> SharedOwned<T> {
    policy
        .admit(IngressClass::Data, plan(16384))
        .unwrap()
        .initialize(create)
}
pub(super) fn own_string(policy: &IngressOwnershipPolicy, value: &str) -> SharedOwned<String> {
    own(policy, || value.to_owned())
}
fn cache(account: &str, lifetime: u64) -> SecretValue {
    let claims = serde_json::json!({"exp": (SystemTime::now() + Duration::from_secs(lifetime))
        .duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(),
        "https://api.openai.com/auth": {"chatgpt_account_id": account, "chatgpt_plan_type":"plus"}});
    SecretValue::new(serde_json::json!({"tokens": {
        "access_token":format!("fixture.{}.fixture", URL_SAFE_NO_PAD.encode(claims.to_string())),
        "refresh_token":"synthetic-refresh", "id_token":"synthetic-id", "account_id":account,
    }}).to_string())
}
pub(super) fn own_cache(
    policy: &IngressOwnershipPolicy,
    account: &str,
    lifetime: u64,
) -> SharedOwned<SecretValue> {
    own(policy, || cache(account, lifetime))
}

#[derive(Clone)]
pub(super) enum Fault {
    Domain(SubscriptionError, bool),
    Source(AdmittedError, bool),
    Immediate(AdmittedError),
}
#[derive(Clone, Default)]
pub(super) struct Config {
    pub operation: Option<IngressOwnershipPolicy>,
    pub envelope: Option<IngressOwnershipPolicy>,
    pub fault: Option<Fault>,
    pub wait: bool,
}
pub(super) struct Boundary {
    pub config: Mutex<Config>,
    calls: AtomicUsize,
    active: AtomicUsize,
    pub release: Semaphore,
}
impl Default for Boundary {
    fn default() -> Self {
        Self {
            config: Mutex::new(Config::default()),
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            release: Semaphore::new(0),
        }
    }
}
struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, SeqCst);
    }
}
impl Boundary {
    pub fn calls(&self) -> usize {
        self.calls.load(SeqCst)
    }
    pub fn active(&self) -> usize {
        self.active.load(SeqCst)
    }
    pub fn operation<'a, T: Send + 'a>(
        &'a self,
        policy: &IngressOwnershipPolicy,
        effect: impl FnOnce() -> Result<T, SubscriptionError> + Send + 'a,
    ) -> Result<SubscriptionOperation<'a, T>, AdmittedError> {
        let config = self.config.lock().unwrap().clone();
        if let Some(Fault::Immediate(error)) = config.fault {
            return Err(error);
        }
        let op_policy = config.operation.as_ref().unwrap_or(policy);
        let scope = IngressWorkScope::new(
            config
                .envelope
                .as_ref()
                .unwrap_or(op_policy)
                .admit(IngressClass::Data, plan(512))?,
        );
        AdmittedOperation::from_owned(
            op_policy,
            IngressClass::Data,
            plan(0),
            move |_| async move {
                self.calls.fetch_add(1, SeqCst);
                self.active.fetch_add(1, SeqCst);
                let _active = Active(&self.active);
                if config.wait {
                    self.release.acquire().await.unwrap().forget();
                }
                match config.fault {
                    Some(Fault::Source(error, committed)) => {
                        if committed {
                            let _ = effect();
                        }
                        Err(error)
                    }
                    Some(Fault::Domain(error, committed)) => {
                        if committed {
                            let _ = effect();
                        }
                        Ok(scope.initialize_owned(|| Err(error)))
                    }
                    Some(Fault::Immediate(_)) => unreachable!(),
                    None => Ok(scope.initialize_owned(effect)),
                }
            },
        )
    }
}

pub(super) struct Store {
    pub record: Mutex<Option<SharedOwned<SubscriptionRecord>>>,
    pub get: Boundary,
    pub claim: Boundary,
    pub finish: Boundary,
    pub reject_finish: AtomicUsize,
}
impl Store {
    pub fn record(&self) -> Option<SharedOwned<SubscriptionRecord>> {
        self.record.lock().unwrap().clone()
    }
}
#[async_trait::async_trait]
impl SubscriptionStore for Store {
    fn get_admitted<'a>(
        &'a self,
        _: &'a SharedOwned<SubjectId>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<SubscriptionOperation<'a, Option<SharedOwned<SubscriptionRecord>>>, AdmittedError>
    {
        self.get.operation(policy, || Ok(self.record()))
    }
    fn replace_admitted<'a>(
        &'a self,
        expected: &'a SharedOwned<SubscriptionRecord>,
        next: &'a SharedOwned<SubscriptionRecord>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<SubscriptionOperation<'a, bool>, AdmittedError> {
        let boundary = if next.refresh_lease.is_some() {
            &self.claim
        } else {
            &self.finish
        };
        boundary.operation(policy, || {
            if next.refresh_lease.is_none() && self.reject_finish.load(SeqCst) != 0 {
                return Ok(false);
            }
            let mut record = self.record.lock().unwrap();
            let Some(current) = &*record else {
                return Ok(false);
            };
            if current.incarnation != expected.incarnation || current.revision != expected.revision
            {
                return Ok(false);
            }
            assert_eq!(current.owner, next.owner);
            assert_eq!(next.revision, current.revision + 1);
            *record = Some(next.clone());
            Ok(true)
        })
    }
    async fn get(&self, _: &SubjectId) -> Result<Option<SubscriptionRecord>, SubscriptionError> {
        panic!("legacy get")
    }
    async fn insert(&self, _: SubscriptionRecord) -> Result<bool, SubscriptionError> {
        panic!("legacy insert")
    }
    async fn replace(
        &self,
        _: &SubscriptionRecord,
        _: SubscriptionRecord,
    ) -> Result<bool, SubscriptionError> {
        panic!("legacy replace")
    }
}

pub(super) struct Native {
    pub boundary: Boundary,
    pub renewed: Mutex<SharedOwned<SecretValue>>,
}
impl Native {
    pub fn calls(&self) -> usize {
        self.boundary.calls()
    }
}
#[async_trait::async_trait]
impl NativeSubscriptionAuth for Native {
    fn refresh_admitted<'a>(
        &'a self,
        _: &'a SharedOwned<SecretValue>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<SubscriptionOperation<'a, SharedOwned<SecretValue>>, AdmittedError> {
        self.boundary
            .operation(policy, || Ok(self.renewed.lock().unwrap().clone()))
    }
    async fn start(&self) -> Result<NativeLogin, SubscriptionError> {
        panic!("legacy start")
    }
    async fn refresh(&self, _: &SecretValue) -> Result<SecretValue, SubscriptionError> {
        panic!("legacy refresh")
    }
}

pub(super) struct Fixture {
    pub root: IngressBudget,
    pub policy: IngressOwnershipPolicy,
    pub owner: SharedOwned<SubjectId>,
    pub store: Arc<Store>,
    pub native: Arc<Native>,
    pub manager: CodexSubscriptions,
}
impl Fixture {
    pub fn new(lifetime: u64) -> Self {
        let (root, policy) = policy();
        Self::with_policy(root, policy, lifetime)
    }
    pub fn with_policy(root: IngressBudget, policy: IngressOwnershipPolicy, lifetime: u64) -> Self {
        let owner = own(&policy, || SubjectId::new("owner"));
        let record = own(&policy, || SubscriptionRecord {
            owner: SubjectId::new("owner"),
            incarnation: "incarnation".into(),
            revision: 1,
            cache: Some(cache("account", lifetime)),
            pending: None,
            refresh_lease: None,
        });
        let store = Arc::new(Store {
            record: Mutex::new(Some(record)),
            get: Boundary::default(),
            claim: Boundary::default(),
            finish: Boundary::default(),
            reject_finish: AtomicUsize::new(0),
        });
        let native = Arc::new(Native {
            boundary: Boundary::default(),
            renewed: Mutex::new(own_cache(&policy, "account", 7200)),
        });
        let manager = CodexSubscriptions {
            store: store.clone(),
            native: native.clone(),
            worker: "synthetic-worker".into(),
            login_capacity: Arc::new(Semaphore::new(1)),
        };
        Self {
            root,
            policy,
            owner,
            store,
            native,
            manager,
        }
    }
    pub fn operation<'a>(
        &'a self,
        force: bool,
        previous: Option<&'a SharedOwned<String>>,
    ) -> Result<SubscriptionOperation<'a, SharedOwned<SubscriptionTokens>>, AdmittedError> {
        self.manager
            .tokens_admitted(&self.owner, force, previous, &self.policy)
    }
    pub async fn tokens(
        &self,
        force: bool,
        previous: Option<&SharedOwned<String>>,
    ) -> Result<Result<SharedOwned<SubscriptionTokens>, SubscriptionError>, AdmittedError> {
        let result = self.operation(force, previous)?.await?;
        assert!(result.shares_custody_domain(&self.policy));
        Ok(result.as_ref().map(Clone::clone).map_err(|error| *error))
    }
    pub fn edit(&self, edit: impl FnOnce(&mut SubscriptionRecord)) {
        let current = self.store.record().unwrap();
        let next = own(&self.policy, || {
            let mut r = (*current).clone();
            edit(&mut r);
            r
        });
        *self.store.record.lock().unwrap() = Some(next);
    }
}

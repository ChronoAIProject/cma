use std::collections::BTreeMap;

use super::*;

pub(super) struct Provider {
    pub bridge: BridgeRecord,
    pub rows: Mutex<BTreeMap<String, StoredInbox>>,
    pub refusals: Mutex<BTreeMap<String, (String, ChannelControlReceipt)>>,
    pub provider_calls: AtomicUsize,
    pub inbox_calls: AtomicUsize,
    pub failure: AtomicUsize,
    pub entered: Notify,
    pub commits: Semaphore,
}

#[async_trait::async_trait]
impl BridgeStore for Provider {
    async fn bridge(&self, id: &BridgeId) -> Result<Option<BridgeRecord>, BridgeError> {
        Ok((id == &self.bridge.id).then(|| self.bridge.clone()))
    }
    async fn receipt(&self, _: &str) -> Result<Option<StoredBridgeReceipt>, BridgeError> {
        unreachable!()
    }
    async fn list(
        &self,
        _: &layer2_core::resource_access::ResourceOwner,
        _: &str,
        _: Option<&BridgeId>,
        _: u32,
    ) -> Result<Vec<BridgeRecord>, BridgeError> {
        unreachable!()
    }
    async fn apply(&self, _: BridgeWrite) -> Result<StoredBridgeReceipt, BridgeError> {
        unreachable!()
    }
}

#[async_trait::async_trait]
impl BridgeConnectionValidator for Provider {
    async fn validate_connection(
        &self,
        owner: &layer2_core::resource_access::ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        self.provider_calls.fetch_add(1, Ordering::SeqCst);
        BridgeValidation::from_observation(
            owner,
            config,
            BridgeConnectionObservation {
                connector: config.connector.clone(),
                bot_owner: owner.clone(),
                key_owner: owner.clone(),
                custody_owner: owner.clone(),
                custody_revision: 1,
                key_state_version: 1,
                routes: vec![BridgeRouteObservation {
                    route_id: "wildcard-route".into(),
                    owner: owner.clone(),
                    bot_id: config.connector.bot_id.clone(),
                    key_id: config.connector.key_id.clone(),
                    enabled: true,
                }],
                key_active: true,
                custody_available: true,
                callback_conflict: false,
                renewable_sandbox_authority: true,
                exact_profiles_available: true,
                existing_target: None,
                deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            },
        )
    }
}

#[async_trait::async_trait]
impl CallbackInboxStore for Provider {
    async fn recover_receipt(
        &self,
        write: &InboxWrite,
    ) -> Result<Option<CallbackAcceptance>, CallbackError> {
        if let Some(receipt) = self
            .rows
            .lock()
            .await
            .get(&write.record().id)
            .map(|row| write.match_replay(row))
            .transpose()?
        {
            return Ok(Some(CallbackAcceptance::Message(receipt)));
        }
        self.refusals
            .lock()
            .await
            .get(&write.record().id)
            .map(|(digest, receipt)| {
                if digest != &write.record().attestation.body_sha256 {
                    return Err(CallbackError::ReplayConflict);
                }
                Ok(CallbackAcceptance::ControlRefused(receipt.clone()))
            })
            .transpose()
    }

    async fn accept(&self, write: InboxWrite) -> Result<CallbackAcceptance, CallbackError> {
        self.inbox_calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.commits.acquire().await.unwrap().forget();
        if self.failure.load(Ordering::SeqCst) == 1 {
            return Err(CallbackError::Unavailable);
        }
        write.require_current(&self.bridge)?;
        let mut rows = self.rows.lock().await;
        if let Some(row) = rows.get(&write.record().id) {
            return write.match_replay(row).map(CallbackAcceptance::Message);
        }
        let mut refusals = self.refusals.lock().await;
        if let Some((digest, receipt)) = refusals.get(&write.record().id) {
            if digest != &write.record().attestation.body_sha256 {
                return Err(CallbackError::ReplayConflict);
            }
            return Ok(CallbackAcceptance::ControlRefused(receipt.clone()));
        }
        if let Some(code) = write.control_refusal() {
            let receipt = ChannelControlReceipt {
                version: 1,
                callback_id: write.record().id.clone(),
                bridge_id: write.record().pin.bridge_id.clone(),
                message_id: write.record().message_id.clone(),
                persisted_at: write.record().accepted_at,
                code,
            };
            refusals.insert(
                write.record().id.clone(),
                (
                    write.record().attestation.body_sha256.clone(),
                    receipt.clone(),
                ),
            );
            if self.failure.load(Ordering::SeqCst) == 2 {
                return Err(CallbackError::Unavailable);
            }
            return Ok(CallbackAcceptance::ControlRefused(receipt));
        }
        let receipt = write.record().receipt();
        if write.record().admission != InboxAdmission::Accepted {
            assert!(write.thread_context().is_none());
            assert!(write.reply_token().is_none());
        }
        rows.insert(write.record().id.clone(), write.record().clone());
        if self.failure.load(Ordering::SeqCst) == 2 {
            return Err(CallbackError::Unavailable);
        }
        Ok(CallbackAcceptance::Message(receipt))
    }
}

pub(super) struct HttpFixture {
    pub protocol: Fixture,
    pub provider: Arc<Provider>,
}

impl HttpFixture {
    pub fn new(organization: bool) -> Self {
        let mut protocol = Fixture::new(BridgePlatform::Telegram);
        protocol.config_mut().connector.callback_url =
            "https://cma.example/api/v1/bridge-callbacks/brg_fixture".into();
        if organization {
            protocol.bridge.owner = layer2_core::resource_access::ResourceOwner::Organization {
                subject: layer2_core::SubjectId::new("fixture-org"),
            };
        }
        let provider = Arc::new(Provider {
            bridge: protocol.bridge.clone(),
            rows: Mutex::new(BTreeMap::new()),
            refusals: Mutex::new(BTreeMap::new()),
            provider_calls: AtomicUsize::new(0),
            inbox_calls: AtomicUsize::new(0),
            failure: AtomicUsize::new(0),
            entered: Notify::new(),
            commits: Semaphore::new(0),
        });
        Self { protocol, provider }
    }
    pub fn request(&self) -> Request<Body> {
        request(&self.protocol)
    }
    pub fn router(&self) -> Router {
        self.router_with_reply_addressing(None)
    }
    pub fn router_with_reply_addressing(
        &self,
        replies: Option<Arc<dyn crate::bridge::ingress::reply_addressing::ReplyAddressingStore>>,
    ) -> Router {
        let mut ingress = CallbackIngress::new(
            self.provider.clone(),
            self.provider.clone(),
            self.provider.clone(),
            PinnedCallbackKeys::fixture(),
            CallbackTokenPolicy {
                issuer: "https://nyx.example".into(),
                relay_audience: "https://nyx.example".into(),
                callback_ttl_seconds: 300,
                relay_ttl_seconds: 300,
                reply_ttl_seconds: 1800,
                clock_skew_seconds: 60,
            },
            "https://cma.example/api/v1/bridge-callbacks".into(),
        )
        .unwrap();
        if let Some(replies) = replies {
            ingress = ingress.with_reply_addressing(replies);
        }
        router(Some(Arc::new(ingress)))
    }
}

pub(super) fn request(protocol: &Fixture) -> Request<Body> {
    let mut request = Request::post("/api/v1/bridge-callbacks/brg_fixture")
        .body(Body::from(protocol.bytes()))
        .unwrap();
    *request.headers_mut() = protocol.headers();
    request
}

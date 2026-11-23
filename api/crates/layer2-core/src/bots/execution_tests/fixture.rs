use super::*;
use crate::{sandbox::binding::ProvidedCredential, SubjectId};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Remote {
    pub keys: BTreeMap<String, (BotKeyRole, String)>,
    pub channel: Option<BotChannelObservation>,
    pub route: Option<(String, String)>,
    pub provider_hash: Option<String>,
    pub calls: Vec<String>,
    pub failure: Option<(&'static str, bool)>,
    pub partial_binding: bool,
    pub enabled: bool,
}
impl Remote {
    fn visit(&mut self, step: &'static str, after: bool) -> Result<(), BotError> {
        if !after {
            self.calls.push(step.into());
        }
        if self.failure == Some((step, after)) {
            self.failure = None;
            Err(BotError::Unavailable)
        } else {
            Ok(())
        }
    }
}
#[derive(Default)]
pub(super) struct External(pub Mutex<Remote>, pub Mutex<Option<Arc<MintPause>>>);

pub(super) struct MintPause {
    pub after: bool,
    pub entered: tokio::sync::Notify,
    pub release: tokio::sync::Notify,
}
impl MintPause {
    pub fn new(after: bool) -> Arc<Self> {
        Arc::new(Self {
            after,
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        })
    }
    async fn wait(&self) {
        self.entered.notify_one();
        self.release.notified().await;
    }
}
#[async_trait::async_trait]
impl BotExternal for External {
    async fn available(&self, _: &SubjectId) -> Result<Vec<ExternalBot>, BotError> {
        Ok(vec![])
    }
    async fn verify(&self, _: &Bot) -> Result<(), BotError> {
        self.0.lock().unwrap().visit("verify", false)
    }
    async fn mint(&self, _: &Bot, role: BotKeyRole) -> Result<BotKey, BotError> {
        let pause = self.1.lock().unwrap().take();
        if let Some(pause) = &pause {
            if !pause.after {
                pause.wait().await;
            }
        }
        let key = {
            let mut remote = self.0.lock().unwrap();
            let name = match role {
                BotKeyRole::Agent => "mint_agent",
                BotKeyRole::Route => "mint_route",
            };
            remote.visit(name, false)?;
            let key = BotKey {
                id: uuid::Uuid::new_v4().to_string(),
                value: ProvidedCredential::new(uuid::Uuid::new_v4().to_string()).unwrap(),
                expires_at: crate::rfc3339_at(SystemTime::now() + Duration::from_secs(30 * 86400)),
            };
            remote
                .keys
                .insert(key.id.clone(), (role, key.value.sha256()));
            remote.visit(name, true)?;
            key
        };
        if let Some(pause) = &pause {
            if pause.after {
                pause.wait().await;
            }
        }
        Ok(key)
    }
    async fn keys(&self, _: &Bot) -> Result<Vec<BotKeyRef>, BotError> {
        let mut remote = self.0.lock().unwrap();
        remote.visit("keys", false)?;
        Ok(remote
            .keys
            .iter()
            .map(|(id, (role, _))| BotKeyRef {
                id: id.clone(),
                role: *role,
            })
            .collect())
    }
    async fn revoke(&self, _: &Bot, id: &str) -> Result<(), BotError> {
        let mut remote = self.0.lock().unwrap();
        remote.visit("revoke", false)?;
        if let Some((_, hash)) = remote.keys.get(id) {
            assert_ne!(
                remote.provider_hash.as_ref(),
                Some(hash),
                "revoked the active provider key"
            );
        }
        assert!(
            remote.route.as_ref().is_none_or(|(_, key)| key != id),
            "revoked a key used by a route"
        );
        remote.keys.remove(id);
        remote.visit("revoke", true)
    }
    async fn create_channel(
        &self,
        bot: &Bot,
        secrets: &BotSecrets,
    ) -> Result<BotChannelObservation, BotError> {
        let mut remote = self.0.lock().unwrap();
        remote.visit("create_channel", false)?;
        let observation = BotChannelObservation {
            channel: BotChannel {
                channel_id: uuid::Uuid::new_v4().to_string(),
                record_id: bot.record_id.clone(),
                version: 1,
                binding_id: "binding".into(),
                level: "l2".into(),
                callback_url: "https://gateway.example/callback".into(),
                event_url: "https://gateway.example/events".into(),
            },
            agent_key_hash: secrets.agent.sha256(),
            route_key_id: bot
                .resources
                .credentials
                .route_key_id
                .clone()
                .unwrap_or_default(),
            route_ids: vec![],
        };
        remote.channel = Some(observation.clone());
        remote.provider_hash = Some(secrets.agent.sha256());
        remote.visit("create_channel", true)?;
        Ok(observation)
    }
    async fn channel(&self, _: &Bot) -> Result<Option<BotChannelObservation>, BotError> {
        let mut remote = self.0.lock().unwrap();
        remote.visit("channel", false)?;
        Ok(remote.channel.clone())
    }
    async fn configure_channel(
        &self,
        bot: &Bot,
        current: &BotChannelObservation,
        keys: &BotCredentialSet,
        _: Option<&BotSecrets>,
        enabled: bool,
    ) -> Result<BotChannelObservation, BotError> {
        let mut remote = self.0.lock().unwrap();
        remote.visit(
            if enabled {
                "configure_enabled"
            } else {
                "configure_disabled"
            },
            false,
        )?;
        let mut next = current.clone();
        assert_eq!(
            remote.channel.as_ref().unwrap().channel.version,
            current.channel.version
        );
        next.channel.version += 1;
        next.agent_key_hash = keys.agent_key_hash.clone().unwrap();
        next.route_key_id = keys.route_key_id.clone().unwrap_or_default();
        next.route_ids = bot.resources.route_id.iter().cloned().collect();
        remote.provider_hash = Some(next.agent_key_hash.clone());
        if std::mem::take(&mut remote.partial_binding) {
            return Err(BotError::Unavailable);
        }
        remote.channel = Some(next.clone());
        remote.enabled = enabled;
        remote.visit(
            if enabled {
                "configure_enabled"
            } else {
                "configure_disabled"
            },
            true,
        )?;
        Ok(next)
    }
    async fn delete_channel(&self, _: &Bot, _: &BotChannelObservation) -> Result<(), BotError> {
        let mut remote = self.0.lock().unwrap();
        remote.visit("delete_channel", false)?;
        remote.channel = None;
        remote.provider_hash = None;
        remote.visit("delete_channel", true)
    }
    async fn set_callback(&self, _: &Bot, _: &str, url: &str) -> Result<(), BotError> {
        let mut remote = self.0.lock().unwrap();
        remote.visit("set_callback", false)?;
        assert_eq!(url, remote.channel.as_ref().unwrap().channel.callback_url);
        remote.visit("set_callback", true)
    }
    async fn create_route(&self, _: &Bot, key: &str) -> Result<String, BotError> {
        let mut remote = self.0.lock().unwrap();
        remote.visit("create_route", false)?;
        let id = uuid::Uuid::new_v4().to_string();
        remote.route = Some((id.clone(), key.into()));
        remote.visit("create_route", true)?;
        Ok(id)
    }
    async fn set_route_key(&self, _: &Bot, route: &str, key: &str) -> Result<(), BotError> {
        let mut remote = self.0.lock().unwrap();
        remote.visit("set_route_key", false)?;
        if remote.route.as_ref().is_none_or(|(id, _)| id != route) {
            return Err(BotError::Unavailable);
        }
        remote.route = Some((route.into(), key.into()));
        remote.visit("set_route_key", true)
    }
    async fn active_route_key(&self, _: &Bot) -> Result<Option<String>, BotError> {
        let mut remote = self.0.lock().unwrap();
        remote.visit("active_route_key", false)?;
        Ok(remote.route.as_ref().map(|(_, key)| key.clone()))
    }
    async fn delete_routes(&self, _: &Bot, keys: &[String]) -> Result<(), BotError> {
        let mut remote = self.0.lock().unwrap();
        remote.visit("delete_routes", false)?;
        if let Some((_, key)) = &remote.route {
            assert!(keys.contains(key));
        }
        remote.route = None;
        remote.visit("delete_routes", true)
    }
    async fn test(&self, _: &Bot) -> Result<BotTestResult, BotError> {
        Err(BotError::Unavailable)
    }
}

pub(super) struct Fixture {
    pub store: Arc<InMemoryBotStore>,
    pub service: Arc<BotService>,
    pub external: Arc<External>,
    pub worker: BotWorker,
    pub bot_id: String,
}
impl Fixture {
    pub async fn new() -> Self {
        Self::with_request(tests::input()).await
    }
    pub async fn with_request(request: CreateBot) -> Self {
        Self::with_policy(request, false).await
    }
    pub async fn fenced() -> Self {
        Self::with_policy(tests::input(), true).await
    }
    pub async fn with_policy(request: CreateBot, fenced: bool) -> Self {
        let store = Arc::new(InMemoryBotStore::default());
        let service = BotService::new(
            store.clone(),
            Arc::new(tests::Profiles),
            "registered".into(),
        )
        .with_effect_reservations(fenced);
        let external = Arc::new(External::default());
        let worker = BotWorker::new(service.clone(), external.clone(), "worker".into());
        let bot = service
            .create(&tests::caller("owner"), "creation", request)
            .await
            .unwrap();
        Self {
            store,
            service,
            external,
            worker,
            bot_id: bot.bot_id,
        }
    }
    pub async fn bot(&self) -> Bot {
        self.store.get(&self.bot_id).await.unwrap().unwrap()
    }
    pub async fn ready(&self) {
        let mut bot = self.bot().await;
        let revision = bot.revision;
        bot.revision += 1;
        bot.work.as_mut().unwrap().expires_at = Some(crate::rfc3339_at(SystemTime::UNIX_EPOCH));
        self.store.replace(revision, &bot).await.unwrap();
    }
    pub async fn renewal_due(&self) {
        let mut bot = self.bot().await;
        let revision = bot.revision;
        bot.revision += 1;
        bot.resources.credentials.expires_at = Some(crate::rfc3339_at(
            SystemTime::now() + Duration::from_secs(86400),
        ));
        self.store.replace(revision, &bot).await.unwrap();
    }
    pub async fn admit(&self, operation: BotOperation) {
        self.service
            .admit(&tests::caller("owner"), &self.bot_id, operation)
            .await
            .unwrap();
    }
    pub fn fail(&self, step: &'static str, after: bool) {
        self.external.0.lock().unwrap().failure = Some((step, after));
    }
}

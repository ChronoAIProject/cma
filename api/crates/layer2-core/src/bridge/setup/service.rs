use super::super::*;
use super::*;
use crate::resource_access::{CallerContext, ResourceOwner};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBridge {
    pub owner: ResourceOwner,
    pub config: BridgeSetupConfig,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateBridge {
    pub expected_revision: u64,
    pub config: BridgeSetupConfig,
    pub previous_work: Option<AcceptedWorkPolicy>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeControlCommand {
    Enable,
    Disable { accepted_work: AcceptedWorkPolicy },
    Retire { accepted_work: AcceptedWorkPolicy },
    Emergency { controls: BridgeEmergencyControls },
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlBridge {
    pub expected_revision: u64,
    pub command: BridgeControlCommand,
}
#[derive(Debug, Clone, Serialize)]
pub struct BridgeConfigurationView {
    pub bridge: BridgeView,
    pub config: BridgeSetupConfig,
    pub callback_url: String,
    pub credential: BridgeCredentialStatus,
}
#[derive(Debug, Clone, Serialize)]
pub struct BridgeValidationView {
    pub bridge_id: BridgeId,
    pub revision: u64,
    pub config_revision: u64,
    pub valid: bool,
}

#[derive(Clone)]
pub struct BridgeSetupService {
    pub(super) definitions: BridgeService,
    policy: BridgeSetupPolicy,
    credentials: Arc<dyn BridgeSetupCredentials>,
    invokes: Option<Arc<super::super::invoke::BridgeInvokeService>>,
}
impl BridgeSetupService {
    pub fn new(
        definitions: BridgeService,
        policy: BridgeSetupPolicy,
        credentials: Arc<dyn BridgeSetupCredentials>,
    ) -> Self {
        Self {
            definitions,
            policy,
            credentials,
            invokes: None,
        }
    }
    pub fn with_invoke_grants(
        mut self,
        invokes: Arc<super::super::invoke::BridgeInvokeService>,
    ) -> Self {
        self.invokes = Some(invokes);
        self
    }
    pub async fn list(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        owner: &ResourceOwner,
        after: Option<&BridgeId>,
        limit: u32,
    ) -> Result<BridgePage, BridgeError> {
        self.definitions
            .list(context, credential, owner, after, limit)
            .await
    }
    pub async fn detail(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &BridgeId,
    ) -> Result<BridgeView, BridgeError> {
        self.definitions.detail(context, credential, id).await
    }
    pub async fn choices(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        owner: &ResourceOwner,
    ) -> Result<BridgeConnectorChoices, BridgeError> {
        let authority = self
            .definitions
            .authority(context, credential, owner, true)
            .await?;
        let platforms = [
            BridgePlatform::Telegram,
            BridgePlatform::Discord,
            BridgePlatform::Lark,
            BridgePlatform::Feishu,
        ];
        if !platforms.iter().any(|platform| {
            self.definitions
                .check_feature(BridgeFeature::Management, platform)
                .is_ok()
        }) {
            return Err(BridgeError::Unavailable);
        }
        let credential = credential.ok_or(BridgeError::AuthorityUnavailable)?;
        let mut choices = tokio::time::timeout(
            Duration::from_secs(5),
            self.credentials.choices(context, credential, owner),
        )
        .await
        .map_err(|_| BridgeError::AuthorityUnavailable)??;
        authority.require(true)?;
        choices.bots.retain(|bot| {
            self.definitions
                .check_feature(BridgeFeature::Management, &bot.platform)
                .is_ok()
        });
        choices.routes.retain(|route| {
            let platform = match route.platform.as_str() {
                "telegram" => BridgePlatform::Telegram,
                "discord" => BridgePlatform::Discord,
                "lark" => BridgePlatform::Lark,
                "feishu" => BridgePlatform::Feishu,
                _ => return false,
            };
            self.definitions
                .check_feature(BridgeFeature::Management, &platform)
                .is_ok()
        });
        let keys: std::collections::BTreeSet<_> = choices
            .routes
            .iter()
            .map(|route| route.key_id.clone())
            .collect();
        choices.keys.retain(|key| keys.contains(&key.id));
        Ok(choices)
    }
    pub async fn create(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        key: String,
        request: CreateBridge,
    ) -> Result<BridgeReceipt, BridgeError> {
        let id = BridgeService::creation_id(context, &key)?;
        let config = self.policy.configure(&id, request.config)?;
        self.definitions
            .mutate(
                context,
                credential,
                BridgeMutation {
                    id,
                    expected_revision: 0,
                    idempotency_key: key,
                    change: BridgeChange::Create {
                        owner: request.owner,
                        config: Box::new(config),
                    },
                },
            )
            .await
    }
    pub async fn update(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: BridgeId,
        key: String,
        request: UpdateBridge,
    ) -> Result<BridgeReceipt, BridgeError> {
        let config = self.policy.configure(&id, request.config)?;
        self.definitions
            .mutate(
                context,
                credential,
                BridgeMutation {
                    id,
                    expected_revision: request.expected_revision,
                    idempotency_key: key,
                    change: BridgeChange::Update {
                        config: Box::new(config),
                        previous_work: request.previous_work,
                    },
                },
            )
            .await
    }
    pub async fn control(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: BridgeId,
        key: String,
        request: ControlBridge,
    ) -> Result<BridgeReceipt, BridgeError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        if matches!(request.command, BridgeControlCommand::Enable) {
            let view = self.definitions.detail(context, credential, &id).await?;
            if !view.can_manage {
                return Err(BridgeError::Forbidden);
            }
            let record = self
                .definitions
                .store
                .bridge(&id)
                .await?
                .ok_or(BridgeError::NotFound)?;
            record.current()?.config.validate_partition_admission()?;
        }
        let change = match request.command {
            BridgeControlCommand::Enable => BridgeChange::Enable,
            BridgeControlCommand::Disable { accepted_work } => {
                BridgeChange::Disable { accepted_work }
            }
            BridgeControlCommand::Retire { accepted_work } => {
                BridgeChange::Retire { accepted_work }
            }
            BridgeControlCommand::Emergency { controls } => BridgeChange::Emergency { controls },
        };
        let mutation = BridgeMutation {
            id: id.clone(),
            expected_revision: request.expected_revision,
            idempotency_key: key,
            change,
        };
        // Ordinary mutation handles exact receipt replay before asking for new
        // consent. Returning a past enable receipt never restores its authority.
        match self
            .definitions
            .mutate(context, credential, mutation.clone())
            .await
        {
            Err(BridgeError::ConnectionRequired(BridgeSetupBlock::InvokeConsentRequired)) => {
                let invokes = self
                    .invokes
                    .as_ref()
                    .ok_or(BridgeError::ConnectionRequired(
                        BridgeSetupBlock::InvokeConsentRequired,
                    ))?;
                let mut management = self
                    .definitions
                    .management_lease(context, credential, &id, request.expected_revision)
                    .await?;
                management.bound_deadline(deadline);
                let proof = invokes.prepare_enable(&management).await?;
                self.definitions
                    .mutate_with_enable_proof(context, credential, mutation, proof)
                    .await
            }
            result => result,
        }
    }
    pub async fn configuration(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &BridgeId,
    ) -> Result<BridgeConfigurationView, BridgeError> {
        let view = self.definitions.detail(context, credential, id).await?;
        let lease = self
            .definitions
            .management_lease(context, credential, id, view.revision)
            .await?;
        let status = self
            .credentials
            .status(&self.definitions, context, credential, id, view.revision)
            .await?;
        self.definitions.validate_management(&lease).await?;
        let config = &lease.record().current()?.config;
        Ok(BridgeConfigurationView {
            bridge: view,
            config: config.into(),
            callback_url: config.connector.callback_url.clone(),
            credential: status,
        })
    }
    pub async fn validate(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &BridgeId,
        expected_revision: u64,
    ) -> Result<BridgeValidationView, BridgeError> {
        let lease = self
            .definitions
            .management_lease(context, credential, id, expected_revision)
            .await?;
        lease
            .record()
            .current()?
            .config
            .validate_partition_admission()?;
        self.definitions.validate(context, credential, id).await?;
        self.definitions.validate_management(&lease).await?;
        Ok(BridgeValidationView {
            bridge_id: id.clone(),
            revision: lease.record().revision,
            config_revision: lease.record().config_revision,
            valid: true,
        })
    }
    pub async fn import(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        request: BridgeCredentialImport,
    ) -> Result<BridgeCredentialStatus, BridgeError> {
        let lease = self
            .definitions
            .management_lease(context, credential, &request.id, request.expected_revision)
            .await?;
        self.definitions.check_feature(
            super::super::BridgeFeature::Management,
            &lease.record().current()?.config.connector.platform,
        )?;
        let result = self
            .credentials
            .import(&self.definitions, context, credential, request)
            .await?;
        self.definitions.validate_management(&lease).await?;
        Ok(result)
    }
    pub async fn revoke(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        request: BridgeCredentialRevocation,
    ) -> Result<BridgeCredentialStatus, BridgeError> {
        let lease = self
            .definitions
            .management_lease(context, credential, &request.id, request.expected_revision)
            .await?;
        let result = self
            .credentials
            .revoke(&self.definitions, context, credential, request)
            .await?;
        self.definitions.validate_management(&lease).await?;
        Ok(result)
    }
}

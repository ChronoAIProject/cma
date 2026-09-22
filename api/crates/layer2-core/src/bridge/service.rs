use std::{sync::Arc, time::Duration};

use crate::resource_access::{CallerContext, OrganizationAuthority, ResourceOwner};

use super::authority::{authorize, BridgeAuthority};
use super::store::digest;
use super::*;

mod connection;
mod placement;

#[derive(Clone)]
pub struct BridgeMutation {
    pub id: BridgeId,
    pub expected_revision: u64,
    pub idempotency_key: String,
    pub change: BridgeChange,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BridgePage {
    pub items: Vec<BridgeView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<BridgeId>,
}

#[derive(Clone)]
pub struct BridgeService {
    pub(super) store: Arc<dyn BridgeStore>,
    organizations: Arc<dyn OrganizationAuthority>,
    connections: Arc<dyn BridgeConnectionValidator>,
}

impl BridgeService {
    /// Share the same fresh connection verifier with trusted reply admission and
    /// delivery composition. This exposes no retained provider credentials.
    pub fn connection_validator(&self) -> Arc<dyn BridgeConnectionValidator> {
        self.connections.clone()
    }
    pub fn check_feature(
        &self,
        feature: BridgeFeature,
        platform: &BridgePlatform,
    ) -> Result<(), BridgeError> {
        self.connections.check_feature(feature, platform)
    }

    pub async fn check_feature_for_pin(
        &self,
        feature: BridgeFeature,
        pin: &BridgeWorkPin,
    ) -> Result<(), BridgeError> {
        let bridge = self
            .store
            .bridge(&pin.bridge_id)
            .await?
            .ok_or(BridgeError::NotFound)?;
        self.check_feature(feature, &bridge.pinned(pin)?.config.connector.platform)
    }
    pub fn new(
        store: Arc<dyn BridgeStore>,
        organizations: Arc<dyn OrganizationAuthority>,
        connections: Arc<dyn BridgeConnectionValidator>,
    ) -> Self {
        Self {
            store,
            organizations,
            connections,
        }
    }

    /// Deterministic identity makes a retried create discover its original
    /// receipt even if the first response was lost. The body is bound separately.
    pub fn creation_id(context: &CallerContext, key: &str) -> Result<BridgeId, BridgeError> {
        validate_key(key)?;
        if !context.validate_principal() {
            return Err(BridgeError::NotFound);
        }
        Ok(BridgeId(format!(
            "brg_{}",
            digest(&(&context.credential.issuer, &context.caller.subject, key))?
        )))
    }

    pub(super) async fn authority(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        owner: &ResourceOwner,
        management: bool,
    ) -> Result<BridgeAuthority, BridgeError> {
        tokio::time::timeout(
            Duration::from_secs(5),
            authorize(
                self.organizations.as_ref(),
                context,
                credential,
                owner,
                management,
            ),
        )
        .await
        .map_err(|_| BridgeError::AuthorityUnavailable)?
    }

    pub async fn detail(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &BridgeId,
    ) -> Result<BridgeView, BridgeError> {
        let record = self.store.bridge(id).await?.ok_or(BridgeError::NotFound)?;
        require_issuer(&record, context)?;
        let authority = self
            .authority(context, credential, &record.owner, false)
            .await?;
        let current = self.store.bridge(id).await?.ok_or(BridgeError::NotFound)?;
        if current.owner != record.owner || current.revision != record.revision {
            return Err(BridgeError::Conflict);
        }
        authority.require(false)?;
        BridgeView::from_record(&current, authority.can_manage)
    }

    pub async fn list(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        owner: &ResourceOwner,
        after: Option<&BridgeId>,
        limit: u32,
    ) -> Result<BridgePage, BridgeError> {
        if !(1..=100).contains(&limit)
            || after.is_some_and(|id| !super::config::valid_id(id.as_str()))
        {
            return Err(BridgeError::InvalidConfiguration("pagination"));
        }
        let authority = self.authority(context, credential, owner, false).await?;
        let mut records = self
            .store
            .list(owner, &context.credential.issuer, after, limit)
            .await?;
        if records.len() > limit as usize + 1 {
            return Err(BridgeError::Corrupt);
        }
        for (index, record) in records.iter().enumerate() {
            record.validate()?;
            require_issuer(record, context)?;
            if &record.owner != owner
                || after.is_some_and(|id| &record.id <= id)
                || (index > 0 && record.id <= records[index - 1].id)
            {
                return Err(BridgeError::Corrupt);
            }
            let current = self
                .store
                .bridge(&record.id)
                .await?
                .ok_or(BridgeError::Conflict)?;
            if current.owner != record.owner || current.revision != record.revision {
                return Err(BridgeError::Conflict);
            }
        }
        let more = records.len() > limit as usize;
        records.truncate(limit as usize);
        let next_cursor = more.then(|| records.last().expect("positive page limit").id.clone());
        let items = records
            .iter()
            .map(|r| BridgeView::from_record(r, authority.can_manage))
            .collect::<Result<_, _>>()?;
        authority.require(false)?;
        Ok(BridgePage { items, next_cursor })
    }

    /// Validation is read-only and does not persist health as future authority.
    pub async fn validate(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &BridgeId,
    ) -> Result<(), BridgeError> {
        let record = self.store.bridge(id).await?.ok_or(BridgeError::NotFound)?;
        require_issuer(&record, context)?;
        let authority = self
            .authority(context, credential, &record.owner, true)
            .await?;
        let validation = self
            .connection(&record.owner, &record.current()?.config)
            .await?;
        let current = self.store.bridge(id).await?.ok_or(BridgeError::NotFound)?;
        if current.revision != record.revision || current.owner != record.owner {
            return Err(BridgeError::Conflict);
        }
        authority.require(true)?;
        validation.require(&record.owner, &record.current()?.config)
    }

    pub async fn mutate(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        mutation: BridgeMutation,
    ) -> Result<BridgeReceipt, BridgeError> {
        self.mutate_checked(context, credential, mutation, None)
            .await
    }

    pub async fn mutate_with_enable_proof(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        mutation: BridgeMutation,
        proof: super::invoke::BridgeInvokeEnableProof,
    ) -> Result<BridgeReceipt, BridgeError> {
        if !matches!(mutation.change, BridgeChange::Enable) {
            return Err(BridgeError::InvalidConfiguration("enable_proof"));
        }
        self.mutate_checked(context, credential, mutation, Some(proof))
            .await
    }

    async fn mutate_checked(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        mutation: BridgeMutation,
        enable_grant: Option<super::invoke::BridgeInvokeEnableProof>,
    ) -> Result<BridgeReceipt, BridgeError> {
        let mut deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        validate_key(&mutation.idempotency_key)?;
        if !super::config::valid_id(mutation.id.as_str()) {
            return Err(BridgeError::InvalidConfiguration("bridge_id"));
        }
        let original = self.store.bridge(&mutation.id).await?;
        if let Some(record) = &original {
            require_issuer(record, context)?;
        }
        let owner = match &original {
            Some(record) => record.owner.clone(),
            None => match &mutation.change {
                BridgeChange::Create { owner, .. } => owner.clone(),
                _ => return Err(BridgeError::NotFound),
            },
        };
        let authority = self.authority(context, credential, &owner, true).await?;
        let connector = match &mutation.change {
            BridgeChange::Create { config, .. } | BridgeChange::Update { config, .. } => {
                &config.connector
            }
            _ => {
                &original
                    .as_ref()
                    .ok_or(BridgeError::NotFound)?
                    .current()?
                    .config
                    .connector
            }
        };
        if connector.issuer != context.credential.issuer {
            return Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::BindingMismatch,
            ));
        }
        if let Some(expiry) = authority.deadline {
            deadline = deadline.min(expiry);
        }
        let receipt_id = digest(&(
            &mutation.id,
            &context.credential.issuer,
            &context.actor,
            &mutation.idempotency_key,
        ))?;
        let command_digest = digest(&(mutation.expected_revision, &mutation.change))?;
        if let Some(prior) = self.store.receipt(&receipt_id).await? {
            authority.require(true)?;
            if prior.receipt.bridge_id != mutation.id
                || prior.receipt.owner != owner
                || prior.receipt.actor != context.actor
                || prior.command_digest != command_digest
            {
                return Err(BridgeError::IdempotencyConflict);
            }
            return Ok(prior.receipt);
        }
        let restores_traffic = match (&mutation.change, &original) {
            (BridgeChange::Emergency { controls }, Some(record)) => {
                (record.emergency.ingress_stopped && !controls.ingress_stopped)
                    || (record.emergency.execution_stopped && !controls.execution_stopped)
                    || (record.emergency.outbound_stopped && !controls.outbound_stopped)
            }
            _ => false,
        };
        if restores_traffic
            || matches!(
                mutation.change,
                BridgeChange::Create { .. } | BridgeChange::Update { .. } | BridgeChange::Enable
            )
        {
            self.check_feature(BridgeFeature::Management, &connector.platform)?;
        }
        self.check_placement_change(original.as_ref(), &mutation.change)?;
        let mut record = plan(original, &mutation, &context.actor)?;
        if matches!(mutation.change, BridgeChange::Enable)
            && matches!(
                record.current()?.config.placement,
                BridgePlacementPolicy::ExistingAgent { .. }
            )
        {
            let proof = enable_grant
                .as_ref()
                .ok_or(BridgeError::ConnectionRequired(
                    BridgeSetupBlock::InvokeConsentRequired,
                ))?;
            proof.require_write(&record, &context.actor, mutation.expected_revision)?;
            deadline = deadline.min(proof.deadline());
        }
        let needs_connection = record.state == BridgeState::Enabled
            && matches!(
                mutation.change,
                BridgeChange::Enable | BridgeChange::Update { .. }
            );
        let custody = if needs_connection {
            let validation = self.connection(&owner, &record.current()?.config).await?;
            deadline = deadline.min(validation.deadline());
            Some(validation.custody_pin().clone())
        } else {
            None
        };
        authority.require(true)?;
        record.updated_at = crate::now_rfc3339();
        let receipt = StoredBridgeReceipt {
            id: receipt_id,
            command_digest,
            receipt: BridgeReceipt {
                bridge_id: record.id.clone(),
                owner: record.owner.clone(),
                actor: context.actor.clone(),
                revision: record.revision,
                config_revision: record.config_revision,
                command: mutation.change.name().into(),
                applied_at: record.updated_at.clone(),
            },
        };
        let write = BridgeWrite {
            expected_revision: mutation.expected_revision,
            record,
            receipt,
            deadline,
            custody,
            enable_grant,
        };
        write.require_current()?;
        Ok(self.store.apply(write).await?.receipt)
    }
}

fn validate_key(key: &str) -> Result<(), BridgeError> {
    if key.is_empty() || key.len() > 200 || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(BridgeError::InvalidConfiguration("idempotency_key"));
    }
    Ok(())
}

fn plan(
    original: Option<BridgeRecord>,
    mutation: &BridgeMutation,
    actor: &crate::resource_access::ResourceActor,
) -> Result<BridgeRecord, BridgeError> {
    let now = crate::now_rfc3339();
    let Some(mut record) = original else {
        let BridgeChange::Create { owner, config } = &mutation.change else {
            return Err(BridgeError::NotFound);
        };
        if mutation.expected_revision != 0 {
            return Err(BridgeError::Conflict);
        }
        config.validate()?;
        let record = BridgeRecord {
            id: mutation.id.clone(),
            owner: owner.clone(),
            revision: 1,
            config_revision: 1,
            state: BridgeState::Disabled {
                accepted_work: AcceptedWorkPolicy::SuspendOriginal,
            },
            emergency: BridgeEmergencyControls::default(),
            created_at: now.clone(),
            updated_at: now.clone(),
            created_by: actor.clone(),
            generations: vec![BridgeGeneration {
                config_revision: 1,
                config: *config.clone(),
                created_at: now,
                actor: actor.clone(),
                previous_work: None,
            }],
        };
        record.validate()?;
        return Ok(record);
    };
    record.validate()?;
    if record.revision != mutation.expected_revision {
        return Err(BridgeError::Conflict);
    }
    if matches!(record.state, BridgeState::Retired { .. })
        && !matches!(mutation.change, BridgeChange::Emergency { .. })
    {
        return Err(BridgeError::Retired);
    }
    match &mutation.change {
        BridgeChange::Create { .. } => return Err(BridgeError::Conflict),
        BridgeChange::Update {
            config,
            previous_work,
        } => {
            config.validate()?;
            let old = &record.current()?.config;
            if old.connector.bot_id != config.connector.bot_id
                || old.connector.platform != config.connector.platform
                || old.connector.issuer != config.connector.issuer
                || old.connector.base_url != config.connector.base_url
            {
                return Err(BridgeError::ImmutableIdentity);
            }
            if old.placement != config.placement
                && (matches!(old.placement, BridgePlacementPolicy::ExistingAgent { .. })
                    || matches!(
                        config.placement,
                        BridgePlacementPolicy::ExistingAgent { .. }
                    ))
                && previous_work.is_none()
            {
                return Err(BridgeError::RebindingPolicyRequired);
            }
            if record.generations.len() >= 256 {
                return Err(BridgeError::HistoryLimit);
            }
            record.config_revision += 1;
            record.generations.push(BridgeGeneration {
                config_revision: record.config_revision,
                config: *config.clone(),
                created_at: now,
                actor: actor.clone(),
                previous_work: *previous_work,
            });
        }
        BridgeChange::Enable => record.state = BridgeState::Enabled,
        BridgeChange::Disable { accepted_work } => {
            record.state = BridgeState::Disabled {
                accepted_work: *accepted_work,
            }
        }
        BridgeChange::Retire { accepted_work } => {
            record.state = BridgeState::Retired {
                accepted_work: *accepted_work,
            }
        }
        BridgeChange::Emergency { controls } => record.emergency = *controls,
    }
    record.revision = record
        .revision
        .checked_add(1)
        .filter(|r| *r <= i64::MAX as u64)
        .ok_or(BridgeError::Conflict)?;
    record.validate()?;
    Ok(record)
}

pub(super) fn require_issuer(
    record: &BridgeRecord,
    context: &CallerContext,
) -> Result<(), BridgeError> {
    if record.current()?.config.connector.issuer != context.credential.issuer {
        return Err(BridgeError::NotFound);
    }
    Ok(())
}

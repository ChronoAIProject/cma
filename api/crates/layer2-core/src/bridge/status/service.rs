use super::*;
use crate::{
    bridge::{BridgeId, BridgePlacementPolicy, BridgeService},
    resource_access::{CallerContext, ResourceAccessService},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct BridgePartitionService {
    pub(super) definitions: BridgeService,
    pub(super) resources: Arc<ResourceAccessService>,
    pub(super) store: Arc<dyn BridgePartitionStore>,
}

impl BridgePartitionService {
    pub fn new(
        definitions: BridgeService,
        resources: Arc<ResourceAccessService>,
        store: Arc<dyn BridgePartitionStore>,
    ) -> Self {
        Self {
            definitions,
            resources,
            store,
        }
    }
    pub(super) async fn management(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
    ) -> Result<BridgeManagementLease, PartitionStatusError> {
        let record = self
            .definitions
            .store
            .bridge(bridge)
            .await?
            .ok_or(PartitionStatusError::NotFound)?;
        Ok(self
            .definitions
            .management_lease(context, credential, bridge, record.revision)
            .await?)
    }
    pub async fn list(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<BridgePartitionPage, PartitionStatusError> {
        tokio::time::timeout(
            Duration::from_secs(15),
            self.list_inner(context, credential, bridge, cursor, limit),
        )
        .await
        .map_err(|_| PartitionStatusError::Unavailable)?
    }
    async fn list_inner(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<BridgePartitionPage, PartitionStatusError> {
        if !(1..=MAX_PARTITION_PAGE).contains(&limit) {
            return Err(PartitionStatusError::Invalid);
        }
        let after = cursor
            .map(|value| decode_cursor(bridge, value))
            .transpose()?;
        let mut manager = self.management(context, credential, bridge).await?;
        let records = self.store.list(&manager, after.as_deref(), limit).await?;
        if records.len() > limit as usize + 1 {
            return Err(PartitionStatusError::Corrupt);
        }
        let mut previous = after.as_deref();
        for evidence in &records {
            validation::allocation(&manager, &evidence.allocation)?;
            if previous.is_some_and(|id| id >= evidence.allocation.id.as_str())
                || evidence.observed_at_ms < 0
            {
                return Err(PartitionStatusError::Corrupt);
            }
            previous = Some(&evidence.allocation.id);
        }
        let mut items = Vec::new();
        let mut checked = Vec::new();
        for evidence in records.iter().take(limit as usize) {
            let access = self
                .authorize_targets(context, credential, &mut manager, evidence)
                .await?;
            items.push(project(&manager, evidence, access.redacted)?);
            checked.push((evidence, access));
        }
        for (evidence, access) in checked {
            self.revalidate_targets(context, credential, &manager, &access)
                .await?;
            self.unchanged(&manager, evidence).await?;
        }
        self.revalidate_management(context, credential, &manager)
            .await?;
        let next_cursor = (records.len() > limit as usize)
            .then(|| encode_cursor(bridge, &records[limit as usize - 1].allocation.id))
            .transpose()?;
        Ok(BridgePartitionPage { items, next_cursor })
    }
    pub async fn detail(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
        id: &str,
    ) -> Result<BridgePartitionDetail, PartitionStatusError> {
        tokio::time::timeout(
            Duration::from_secs(15),
            self.detail_inner(context, credential, bridge, id),
        )
        .await
        .map_err(|_| PartitionStatusError::Unavailable)?
    }
    async fn detail_inner(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
        id: &str,
    ) -> Result<BridgePartitionDetail, PartitionStatusError> {
        if !allocation_id(id) {
            return Err(PartitionStatusError::Invalid);
        }
        let mut manager = self.management(context, credential, bridge).await?;
        let evidence = self
            .store
            .detail(&manager, id)
            .await?
            .ok_or(PartitionStatusError::NotFound)?;
        validation::allocation(&manager, &evidence.allocation)?;
        if evidence.allocation.id != id || evidence.observed_at_ms < 0 {
            return Err(PartitionStatusError::Corrupt);
        }
        let access = self
            .authorize_targets(context, credential, &mut manager, &evidence)
            .await?;
        let partition = project(&manager, &evidence, access.redacted)?;
        let agent = access
            .checked
            .iter()
            .find_map(|(_, _, access)| access.snapshot.lineage.agent.as_ref());
        let activity = if access.redacted {
            PartitionActivityView::Redacted
        } else if let Some(agent) = agent {
            let records = self.store.activity(&manager, &evidence.allocation).await?;
            activity_view(manager.record(), &evidence.allocation, agent, records)?
        } else {
            PartitionActivityView::Unavailable
        };
        self.revalidate_targets(context, credential, &manager, &access)
            .await?;
        self.unchanged(&manager, &evidence).await?;
        self.revalidate_management(context, credential, &manager)
            .await?;
        Ok(BridgePartitionDetail {
            partition,
            activity,
        })
    }
    async fn unchanged(
        &self,
        manager: &BridgeManagementLease,
        original: &PartitionEvidence,
    ) -> Result<(), PartitionStatusError> {
        let current = self
            .store
            .detail(manager, &original.allocation.id)
            .await?
            .ok_or(PartitionStatusError::Conflict)?;
        validation::allocation(manager, &current.allocation)?;
        if current.allocation != original.allocation
            || current.current != original.current
            || current.present != original.present
            || current.first_source != original.first_source
        {
            return Err(PartitionStatusError::Conflict);
        }
        Ok(())
    }

    pub(super) async fn revalidate_management(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        original: &BridgeManagementLease,
    ) -> Result<(), PartitionStatusError> {
        original.require_current()?;
        let current = self
            .definitions
            .management_lease(
                context,
                credential,
                &original.record().id,
                original.record().revision,
            )
            .await?;
        if current.record() != original.record() {
            return Err(PartitionStatusError::Conflict);
        }
        original.require_current()?;
        Ok(())
    }
}

fn project(
    manager: &BridgeManagementLease,
    evidence: &PartitionEvidence,
    redacted: bool,
) -> Result<BridgePartitionView, PartitionStatusError> {
    let record = &evidence.allocation;
    let source = evidence
        .first_source
        .as_ref()
        .map(|inbox| validation::first_source(manager.record(), record, inbox))
        .transpose()?;
    Ok(BridgePartitionView {
        id: record.id.clone(),
        bridge_id: record.identity.bridge_id.clone(),
        generation: record.identity.generation,
        revision: record.revision,
        config_revision: record.pin.config_revision,
        current: evidence.current,
        state: record.state,
        placement: match record.config.placement {
            BridgePlacementPolicy::ExistingAgent { .. } => PartitionPlacement::ExistingAgent,
            BridgePlacementPolicy::NewSandbox { .. } => PartitionPlacement::NewSandbox,
            BridgePlacementPolicy::WorkspacePerPartition { .. } => {
                PartitionPlacement::WorkspacePerPartition
            }
            BridgePlacementPolicy::WorktreeAgentPerPartition { .. } => {
                PartitionPlacement::WorktreeAgentPerPartition
            }
        },
        context_sharing: record.config.partition.into(),
        target: if redacted {
            PartitionTargetView::Redacted
        } else if evidence.present.is_empty() {
            PartitionTargetView::Reserved {
                ids: record.placement.target().clone(),
            }
        } else {
            PartitionTargetView::Available {
                ids: record.placement.target().clone(),
                present: evidence.present.clone(),
            }
        },
        source: if redacted {
            PartitionSourceView::Redacted
        } else {
            source.unwrap_or(PartitionSourceView::Unavailable)
        },
        observed_at_ms: evidence.observed_at_ms,
    })
}

fn activity_view(
    bridge: &crate::bridge::BridgeRecord,
    allocation: &crate::bridge::allocation::AllocationRecord,
    agent: &crate::sandbox::Agent,
    records: PartitionActivityEvidence,
) -> Result<PartitionActivityView, PartitionStatusError> {
    if records.pending.len() > MAX_PARTITION_PENDING + 1 || records.observed_at_ms < 0 {
        return Err(PartitionStatusError::Corrupt);
    }
    let mut previous = 0;
    let mut pending = Vec::new();
    let mut unavailable = false;
    for (index, evidence) in records.pending.iter().enumerate() {
        let order = evidence.command.receipt.agent_order;
        if evidence.command.status.is_terminal() || order <= previous {
            return Err(PartitionStatusError::Corrupt);
        }
        previous = order;
        let view = validation::command(bridge, allocation, evidence, agent)?;
        unavailable |= view.is_none();
        if index < MAX_PARTITION_PENDING {
            pending.extend(view);
        }
    }
    let latest = records
        .latest
        .as_ref()
        .map(|evidence| validation::command(bridge, allocation, evidence, agent))
        .transpose()?
        .flatten();
    unavailable |= records.latest.is_some() && latest.is_none();
    if let Some(latest) = &records.latest {
        if latest.command.receipt.agent_order < previous {
            return Err(PartitionStatusError::Corrupt);
        }
        if let Some(pending) = records.pending.iter().find(|v| {
            v.command.receipt.envelope.command_id == latest.command.receipt.envelope.command_id
        }) {
            if pending.command != latest.command
                || pending.sidecar != latest.sidecar
                || pending.source != latest.source
            {
                return Err(PartitionStatusError::Corrupt);
            }
        }
    } else if !records.pending.is_empty() {
        return Err(PartitionStatusError::Corrupt);
    }
    Ok(PartitionActivityView::Available {
        agent_status: agent.status,
        pending,
        pending_truncated: records.pending.len() > MAX_PARTITION_PENDING,
        source_unavailable: unavailable,
        latest,
        observed_at_ms: records.observed_at_ms,
    })
}

fn allocation_id(value: &str) -> bool {
    value.strip_prefix("bral_").is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn encode_cursor(bridge: &BridgeId, id: &str) -> Result<String, PartitionStatusError> {
    serde_json::to_vec(&("cma.bridge.partitions.v1", bridge, id))
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| PartitionStatusError::Corrupt)
}
fn decode_cursor(bridge: &BridgeId, value: &str) -> Result<String, PartitionStatusError> {
    if value.is_empty() || value.len() > 1024 {
        return Err(PartitionStatusError::Invalid);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| PartitionStatusError::Invalid)?;
    let (version, scope, id): (String, BridgeId, String) =
        serde_json::from_slice(&bytes).map_err(|_| PartitionStatusError::Invalid)?;
    if version != "cma.bridge.partitions.v1"
        || scope != *bridge
        || !allocation_id(&id)
        || encode_cursor(bridge, &id)? != value
    {
        return Err(PartitionStatusError::Invalid);
    }
    Ok(id)
}

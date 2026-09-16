use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

pub(super) fn allocation_id(value: &str) -> bool {
    value.strip_prefix("bral_").is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
pub(super) fn cursor_after(value: &str, bridge: &str) -> Result<String> {
    if value.is_empty() || value.len() > 1024 {
        return Err(Error::protocol("invalid partition cursor"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| Error::protocol("invalid partition cursor"))?;
    let (version, scope, id): (String, String, String) =
        serde_json::from_slice(&bytes).map_err(|_| Error::protocol("invalid partition cursor"))?;
    if version != "cma.bridge.partitions.v1"
        || scope != bridge
        || !allocation_id(&id)
        || URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&(&version, &scope, &id))
                .map_err(|_| Error::protocol("invalid partition cursor"))?,
        ) != value
    {
        return Err(Error::protocol(
            "partition cursor changed scope or encoding",
        ));
    }
    Ok(id)
}
fn opaque(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn number(value: u64) -> bool {
    value > 0 && value <= i64::MAX as u64
}
fn resource_order(resource: BridgePartitionResource) -> u8 {
    match resource {
        BridgePartitionResource::Sandbox => 0,
        BridgePartitionResource::Workspace => 1,
        BridgePartitionResource::Worktree => 2,
        BridgePartitionResource::Agent => 3,
    }
}

impl BridgePartition {
    fn check(&self, bridge_id: &str) -> Result<()> {
        if !allocation_id(&self.id)
            || self.bridge_id != bridge_id
            || !(1..=256).contains(&self.generation)
            || !(1..=256).contains(&self.config_revision)
            || !number(self.revision)
            || self.observed_at_ms > i64::MAX as u64
        {
            return Err(Error::protocol("invalid partition identity or revision"));
        }
        if matches!(self.target, BridgePartitionTarget::Redacted { .. })
            != matches!(self.source, BridgePartitionSource::Redacted { .. })
        {
            return Err(Error::protocol("partition redaction disagrees"));
        }
        let ids = match &self.target {
            BridgePartitionTarget::Available { ids, present } => {
                if present.is_empty()
                    || present.len() > 4
                    || present
                        .windows(2)
                        .any(|pair| resource_order(pair[0]) >= resource_order(pair[1]))
                {
                    return Err(Error::protocol(
                        "partition resources are not bounded and ordered",
                    ));
                }
                Some(ids)
            }
            BridgePartitionTarget::Reserved { ids } => Some(ids),
            BridgePartitionTarget::Redacted { .. } => None,
        };
        if ids.is_some_and(|ids| {
            [
                &ids.sandbox_id,
                &ids.workspace_id,
                &ids.worktree_id,
                &ids.agent_id,
            ]
            .iter()
            .any(|id| !bridge::valid_id(id))
        }) {
            return Err(Error::protocol("invalid partition target"));
        }
        if let BridgePartitionSource::Available {
            chat_id,
            sender_id,
            sender_display_name,
            ..
        } = &self.source
            && (!opaque(chat_id)
                || !opaque(sender_id)
                || sender_display_name.as_ref().is_some_and(|name| {
                    name.trim().is_empty()
                        || name.chars().count() > 256
                        || name.chars().any(char::is_control)
                }))
        {
            return Err(Error::protocol("invalid partition source label"));
        }
        Ok(())
    }
}
impl BridgePartitionPage {
    pub(super) fn check(&self, bridge: &str, after: Option<&str>, limit: usize) -> Result<()> {
        if self.items.len() > limit {
            return Err(Error::protocol("partition page exceeds requested limit"));
        }
        let mut previous = after;
        for item in &self.items {
            item.check(bridge)?;
            if previous.is_some_and(|id| id >= item.id.as_str()) {
                return Err(Error::protocol("partition page is not strictly ordered"));
            }
            previous = Some(&item.id);
        }
        if let Some(cursor) = &self.next_cursor
            && (self.items.is_empty() || Some(cursor_after(cursor, bridge)?.as_str()) != previous)
        {
            return Err(Error::protocol(
                "partition continuation does not match page",
            ));
        }
        Ok(())
    }
}
impl BridgePartitionCommand {
    fn terminal(&self) -> bool {
        matches!(
            self.status,
            BridgePartitionResponseStatus::Completed
                | BridgePartitionResponseStatus::Failed
                | BridgePartitionResponseStatus::Cancelled
        )
    }
    fn check(&self) -> Result<()> {
        if !bridge::valid_id(&self.command_id)
            || !bridge::valid_id(&self.response_id)
            || !(1..=256).contains(&self.config_revision)
            || !number(self.agent_order)
            || self.turn_sequence.is_some_and(|turn| !number(turn))
            || self.admitted_at.trim().is_empty()
            || self.admitted_at.len() > 64
            || self.admitted_at.chars().any(char::is_control)
            || (self.status == BridgePartitionResponseStatus::WaitingAction)
                != (self.waiting_for != BridgePartitionWait::None)
            || (self.waiting_for == BridgePartitionWait::Worker
                && (self.turn_sequence.is_some() || self.delivery.is_some()))
            || (self.waiting_for == BridgePartitionWait::Approval && self.turn_sequence.is_none())
        {
            return Err(Error::protocol("inconsistent partition command"));
        }
        Ok(())
    }
}
impl BridgePartitionDetail {
    pub(super) fn check(&self, bridge: &str, id: &str) -> Result<()> {
        self.partition.check(bridge)?;
        if self.partition.id != id
            || matches!(
                self.partition.target,
                BridgePartitionTarget::Redacted { .. }
            ) != matches!(self.activity, BridgePartitionActivity::Redacted { .. })
        {
            return Err(Error::protocol(
                "partition detail changed identity or redaction",
            ));
        }
        if let BridgePartitionActivity::Available {
            pending,
            pending_truncated,
            source_unavailable,
            latest,
            observed_at_ms,
            ..
        } = &self.activity
        {
            if !matches!(&self.partition.target, BridgePartitionTarget::Available { present, .. } if present.contains(&BridgePartitionResource::Agent))
                || *observed_at_ms > i64::MAX as u64
                || pending.len() > 64
                || (*pending_truncated && pending.len() < 64 && !source_unavailable)
                || (latest.is_none() && !pending.is_empty() && !source_unavailable)
            {
                return Err(Error::protocol(
                    "partition activity lacks target or bounded evidence",
                ));
            }
            let mut previous = 0;
            let mut commands = std::collections::BTreeSet::new();
            let mut responses = std::collections::BTreeSet::new();
            for command in pending {
                command.check()?;
                if command.terminal()
                    || command.agent_order <= previous
                    || !commands.insert(&command.command_id)
                    || !responses.insert(&command.response_id)
                {
                    return Err(Error::protocol(
                        "pending Channel commands are not unique and ordered",
                    ));
                }
                previous = command.agent_order;
            }
            if let Some(latest) = latest {
                latest.check()?;
                if latest.agent_order < previous
                    || pending
                        .iter()
                        .any(|command| command.command_id == latest.command_id && command != latest)
                {
                    return Err(Error::protocol(
                        "partition latest response disagrees with pending work",
                    ));
                }
            }
        }
        Ok(())
    }
}

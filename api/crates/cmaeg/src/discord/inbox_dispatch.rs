//! Native work enters only the durable routed admission path; never direct execution.
use super::{
    inbox_scan::{Disposition, Entry, InboxStore},
    runtime_source, GatewayError, InboxLaunch, InboxReader,
};
use crate::{
    ingress::{launch::LaunchOutcome, IngressState},
    records::RecordClient,
};
use cmaeg_core::seal::MasterKeyRing;
use std::{collections::BTreeSet, sync::Arc};

pub struct InboxDispatcher {
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    ingress: IngressState,
    selected: super::selection::Selection,
    reader: InboxReader,
    store: InboxStore,
}
pub struct DispatchPage {
    pub next_cursor: Option<String>,
    pub acknowledged: usize,
    pub pending: usize,
    pub invalid: usize,
}
impl InboxDispatcher {
    pub fn new(
        records: RecordClient,
        keys: Arc<MasterKeyRing>,
        ingress: IngressState,
        selected: BTreeSet<String>,
    ) -> Self {
        Self::with_selection(
            records,
            keys,
            ingress,
            super::selection::Selection::new(&crate::NativeDiscordConfig {
                enabled: true,
                discovery_enabled: false,
                rich_content_enabled: false,
                forwards_enabled: false,
                records: selected,
            }),
        )
    }
    pub(crate) fn with_selection(
        records: RecordClient,
        keys: Arc<MasterKeyRing>,
        ingress: IngressState,
        selected: super::selection::Selection,
    ) -> Self {
        Self {
            reader: InboxReader::new(records.clone(), keys.clone()),
            store: InboxStore::new(records.clone(), keys.clone()),
            records,
            keys,
            ingress,
            selected,
        }
    }
    pub async fn page(&self, after: Option<&str>, now: i64) -> Result<DispatchPage, GatewayError> {
        if self.selected.is_empty() {
            return Ok(DispatchPage {
                next_cursor: None,
                acknowledged: 0,
                pending: 0,
                invalid: 0,
            });
        }
        let page = self.store.page(after, now).await?;
        let mut report = DispatchPage {
            next_cursor: page.next_cursor,
            acknowledged: 0,
            pending: 0,
            invalid: page.invalid,
        };
        for entry in page.entries {
            if !self.selected.contains(&entry.inbox.channel_record) {
                continue;
            }
            match self.dispatch(&entry, now).await {
                Ok(true) => report.acknowledged += 1,
                Ok(false)
                | Err(
                    GatewayError::Unavailable | GatewayError::RateLimited | GatewayError::Authority,
                ) => {
                    report.pending += 1;
                    self.selected.status().dispatch(
                        &entry.inbox.channel_record,
                        &entry.inbox.source_revision,
                        "pending",
                    );
                }
                Err(_) => {
                    report.invalid += 1;
                    self.selected.status().dispatch(
                        &entry.inbox.channel_record,
                        &entry.inbox.source_revision,
                        "invalid",
                    );
                }
            }
        }
        Ok(report)
    }
    async fn dispatch(&self, entry: &Entry, now: i64) -> Result<bool, GatewayError> {
        if self.store.settled(entry).await? {
            return Ok(true);
        }
        let current = runtime_source::resolve(
            &self.records,
            &self.keys,
            &self.ingress.resolver,
            &entry.inbox.channel_record,
        )
        .await?;
        if current.channel.definition.owner.subject != entry.owner
            || current.revision != entry.inbox.source_revision
        {
            return Err(GatewayError::Authority);
        }
        if entry.inbox.kind == "discord_reaction_pending_v1"
            && now >= entry.inbox.received_at + super::reaction_pending::TARGET_WAIT_SECONDS
        {
            let identity = crate::ingress::routing_store::EventIdentity {
                owner: entry.owner.clone(),
                channel_id: current.channel.id.clone(),
                record_id: current.channel.definition.record_id.clone(),
                event_id: cmaeg_core::ids::EventId::new(
                    entry
                        .inbox
                        .change_id
                        .as_deref()
                        .ok_or(GatewayError::Protocol)?,
                )
                .map_err(|_| GatewayError::Protocol)?,
            };
            // A lost acknowledgement cannot relabel a committed admission as a timeout.
            let plans = crate::ingress::routing_store::PlanStore::new(
                self.records.clone(),
                self.keys.clone(),
            );
            let disposition = match plans
                .load(&identity)
                .await
                .map_err(|_| GatewayError::Unavailable)?
            {
                Some(plan) => Disposition::Admitted {
                    admission_id: plan.admission_id,
                },
                None => {
                    self.ingress.activity.refused(
                        &identity.record_id,
                        "discord",
                        Some(&identity.event_id),
                        "discord_reaction_target_confirmation_timed_out",
                    );
                    Disposition::Refused {
                        reason: "discord_reaction_target_confirmation_timed_out".into(),
                    }
                }
            };
            self.store.acknowledge(entry, disposition).await?;
            return Ok(true);
        }
        // Repair a crash between inbox commit and Activity persistence before
        // checking launch capacity or expiring the original reply authority.
        self.reader
            .observe(
                &entry.id,
                &current.channel,
                now,
                self.ingress.activity.as_ref(),
            )
            .await?;
        let disposition = if entry.expired(now) {
            Disposition::Expired
        } else {
            match self.reader.load(&entry.id, current.channel, now).await? {
                InboxLaunch::Refused(reason) => Disposition::Refused {
                    reason: serde_json::to_value(reason)
                        .map_err(|_| GatewayError::Protocol)?
                        .as_str()
                        .ok_or(GatewayError::Protocol)?
                        .into(),
                },
                InboxLaunch::Launch(verified) => {
                    if entry.inbox.kind == "discord_reaction_pending_v1" {
                        self.ingress
                            .activity
                            .receive_at(
                                &entry.owner,
                                &verified.request.channel.definition.record_id,
                                &verified.request.activity,
                                entry.inbox.received_at,
                            )
                            .await
                            .map_err(|_| GatewayError::Unavailable)?;
                    }
                    let permit = self
                        .ingress
                        .inflight
                        .clone()
                        .try_acquire_owned()
                        .map_err(|_| GatewayError::Unavailable)?;
                    let outcome = if let Some(original) = verified.original_message {
                        self.ingress
                            .launch_message_event_verified(
                                crate::ingress::message_event::VerifiedMessageEvent {
                                    request: verified.request,
                                    source_bytes: verified.source_bytes,
                                    received_at: verified.received_at,
                                    original,
                                    outbound_consumer: verified.outbound_consumer,
                                },
                                permit,
                            )
                            .await
                    } else {
                        self.ingress.launch_native_verified(*verified, permit).await
                    };
                    match outcome {
                        LaunchOutcome::Admitted { admission_id, .. } => {
                            Disposition::Admitted { admission_id }
                        }
                        LaunchOutcome::Refused(problem)
                            if matches!(problem.status, 400 | 403 | 409 | 410 | 413 | 422) =>
                        {
                            Disposition::Refused {
                                reason: format!("admission_refused_{}", problem.status),
                            }
                        }
                        LaunchOutcome::Refused(_) => return Ok(false),
                        LaunchOutcome::Accepted { .. } | LaunchOutcome::Deferred => {
                            return Err(GatewayError::Protocol);
                        }
                    }
                }
            }
        };
        // A lost acknowledgement is read back as the exact immutable value.
        // If it is absent, retry reopens the original routed admission safely.
        let outcome = match &disposition {
            Disposition::Admitted { .. } => "admitted",
            Disposition::Refused { .. } => "refused",
            Disposition::Expired => "expired",
        };
        self.store.acknowledge(entry, disposition).await?;
        self.selected.status().dispatch(
            &entry.inbox.channel_record,
            &entry.inbox.source_revision,
            outcome,
        );
        Ok(true)
    }
}

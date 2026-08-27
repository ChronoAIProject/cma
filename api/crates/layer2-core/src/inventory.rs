//! What is actually running, and whose it is.
//!
//! Two sources, joined. The providers know which runtimes exist — they are the
//! only ones who do, since a sandbox outlives the process that asked for it —
//! and the record knows which session each belongs to and who owns it. Neither
//! answer is complete on its own: a provider's list is anonymous, and the
//! record describes what *should* be running rather than what is.
//!
//! **Not knowing is not the same as nothing being there.** A provider that
//! cannot enumerate says so, and this reports that refusal rather than folding
//! it into an empty list. The distinction is the whole point of the type: an
//! operator shown "nothing is running" when the truth is "nobody could be
//! asked" will conclude the fleet is idle, and act on it.

use std::collections::{HashMap, HashSet};

use crate::authz::{Caller, Capability};
use crate::coordination::{RuntimeAuthority, RuntimeCleanup, RuntimeCleanupReason};
use crate::error::CoreError;
use crate::ids::{KernelId, SessionId, SubjectId};
use crate::service::{store_failed, SessionService};
use crate::session::SessionStatus;
use crate::store::{SkillPackState, StoreError};

/// What provider metadata says about a runtime's durable generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderRuntimeGeneration {
    /// The runtime predates generation metadata.
    Legacy,
    /// The provider returned a valid non-zero generation.
    Recorded(u64),
    /// Metadata was present but could not name a valid generation.
    Malformed,
}

impl From<layer3_adapter::RuntimeGeneration> for ProviderRuntimeGeneration {
    fn from(generation: layer3_adapter::RuntimeGeneration) -> Self {
        match generation {
            layer3_adapter::RuntimeGeneration::Legacy => Self::Legacy,
            layer3_adapter::RuntimeGeneration::Recorded(generation) => {
                Self::Recorded(generation.get())
            }
            layer3_adapter::RuntimeGeneration::Malformed => Self::Malformed,
        }
    }
}

/// Whether provider metadata names the authority durable in the session store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeAuthorityComparison {
    /// The provider names a session for which no durable record exists.
    Unaccounted,
    /// This deployment is still on a store protocol without runtime authority.
    CoordinationDisabled,
    /// Both sides name the same legacy handle, but neither can prove a generation.
    Legacy,
    /// Provider handle and non-zero generation exactly match durable authority.
    Matches,
    /// The provider handle or generation differs from durable authority.
    Mismatch,
}

/// One runtime, and what the record knows about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeEntry {
    /// The provider's own identifier for it. Opaque; never parsed.
    pub runtime: String,
    /// Durable generation recorded by the provider, when this runtime has one.
    ///
    /// Absence marks a legacy runtime. It must stay visible as absence so an
    /// operator or reclamation path cannot mistake it for current authority.
    pub generation: ProviderRuntimeGeneration,
    /// Runtime authority read from the durable coordination record, when enabled.
    pub durable_authority: Option<RuntimeAuthority>,
    /// Exact comparison between provider metadata and durable authority.
    pub authority_comparison: RuntimeAuthorityComparison,
    /// Why this exact provider identity remains durably scheduled for cleanup.
    pub cleanup_reason: Option<RuntimeCleanupReason>,
    /// Which control plane created it, when the provider records one.
    ///
    /// A provider is shared, so an operator reading this may be looking at
    /// sandboxes belonging to a deployment other than the one they are asking.
    /// Saying which is what stops a stranger's runtime reading as an orphan.
    pub created_by: Option<String>,
    /// The session it was provisioned for, as the provider recorded it.
    pub session: Option<SessionId>,
    /// Who owns that session, when the record still knows of it.
    pub owner: Option<SubjectId>,
    /// What state the record says that session is in.
    pub status: Option<SessionStatus>,
}

impl RuntimeEntry {
    /// Whether nothing in the record accounts for this runtime.
    ///
    /// Either it names no session, or it names one nothing has heard of. Both
    /// mean the same thing to whoever is looking at it: something is running
    /// that nobody is entitled to. Reclamation still requires exact durable
    /// cleanup debt and generation proof.
    pub fn is_unaccounted(&self) -> bool {
        self.owner.is_none()
    }
}

/// What one provider had to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderView {
    /// It answered, and this is what it is running.
    Observed(Vec<RuntimeEntry>),
    /// It cannot be asked, for the stated reason.
    ///
    /// Carried as a variant rather than an empty list plus a flag, so that a
    /// surface rendering this has to acknowledge the case to compile.
    Unobservable(String),
}

/// One provider's contribution to the inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInventory {
    /// Which runtime implementation this is.
    pub kernel: KernelId,
    /// What it said.
    pub view: ProviderView,
}

/// Everything in service right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeInventory {
    /// One per runtime implementation this deployment can provision.
    pub providers: Vec<ProviderInventory>,
}

impl RuntimeInventory {
    /// How many runtimes are in service, when that can be said at all.
    ///
    /// `None` as soon as one provider could not be asked. A partial total
    /// presented as a total is the same lie as an empty list: it invites
    /// somebody to conclude that capacity is free.
    pub fn running(&self) -> Option<usize> {
        self.providers
            .iter()
            .map(|p| match &p.view {
                ProviderView::Observed(runtimes) => Some(runtimes.len()),
                ProviderView::Unobservable(_) => None,
            })
            .sum()
    }

    /// Whether every provider could be asked.
    pub fn fully_observed(&self) -> bool {
        self.providers
            .iter()
            .all(|p| matches!(p.view, ProviderView::Observed(_)))
    }

    /// Every runtime nothing in the record accounts for.
    pub fn unaccounted(&self) -> impl Iterator<Item = &RuntimeEntry> {
        self.providers
            .iter()
            .filter_map(|p| match &p.view {
                ProviderView::Observed(runtimes) => Some(runtimes.iter()),
                ProviderView::Unobservable(_) => None,
            })
            .flatten()
            .filter(|r| r.is_unaccounted())
    }
}

impl SessionService {
    /// What every provider is running, joined against the record.
    ///
    /// Asks the providers rather than the cluster: the control plane holds no
    /// right to enumerate anything in the namespace it runs in, and does not
    /// need one — it provisioned every sandbox through an adapter, and the
    /// adapter is what can be asked what became of them.
    pub async fn runtime_inventory(&self, caller: &Caller) -> Result<RuntimeInventory, CoreError> {
        caller
            .authorization
            .require(Capability::ReadRuntimeInventory)?;

        enum ListedProviderView {
            Observed(Vec<layer3_adapter::RuntimeListing>),
            Unobservable(String),
        }

        struct ListedProvider {
            kernel: KernelId,
            view: ListedProviderView,
        }

        // The provider rows define the only durable sessions this response can
        // reference. Keep those rows, then exact-read each unique identifier
        // once. A failed durable read still discards every provider observation
        // rather than returning anonymous runtimes.
        let mut referenced_sessions = HashSet::new();
        let mut listed_providers = Vec::with_capacity(self.adapters.len());
        for (kernel, adapter) in &self.adapters {
            let view = match adapter.list_runtimes().await {
                Ok(runtimes) => {
                    referenced_sessions.extend(
                        runtimes
                            .iter()
                            .filter_map(|runtime| runtime.session.as_ref().cloned()),
                    );
                    ListedProviderView::Observed(runtimes)
                }
                Err(error) => {
                    tracing::warn!(
                        kernel = %kernel,
                        error = %error,
                        "a provider cannot say what it is running; the inventory will say so"
                    );
                    ListedProviderView::Unobservable(error.to_string())
                }
            };
            listed_providers.push(ListedProvider {
                kernel: kernel.clone(),
                view,
            });
        }

        enum AuthorityKnowledge {
            CoordinationDisabled,
            Present(RuntimeAuthority),
        }

        struct KnownSession {
            kernel: KernelId,
            owner: SubjectId,
            status: SessionStatus,
            authority: AuthorityKnowledge,
            cleanup_debts: Vec<RuntimeCleanup>,
        }

        let mut known_sessions = HashMap::with_capacity(referenced_sessions.len());
        for referenced in referenced_sessions {
            let id = SessionId(referenced);
            let record = match self.store.get(&id).await {
                Ok(Some(record)) if record.session.id != id => {
                    return Err(store_failed(StoreError::Malformed(format!(
                        "session point read for {id} returned {}",
                        record.session.id
                    ))))
                }
                Ok(Some(record))
                    if matches!(&record.skill_packs, SkillPackState::StagedSummary) =>
                {
                    return Err(store_failed(StoreError::Malformed(format!(
                        "exact session read for {id} returned a projected skill custody summary"
                    ))))
                }
                Ok(Some(record)) => record,
                Ok(None) => continue,
                Err(error) => return Err(store_failed(error)),
            };
            let (authority, cleanup_debts) =
                match self.store.runtime_authority(&record.session.id).await {
                    Ok(Some(authority)) => (
                        AuthorityKnowledge::Present(authority),
                        self.store
                            .runtime_cleanup_debts(&record.session.id)
                            .await
                            .map_err(store_failed)?,
                    ),
                    Ok(None) => {
                        return Err(CoreError::RuntimeUnavailable(format!(
                            "the runtime authority for {} disappeared while inventory was read",
                            record.session.id
                        )))
                    }
                    Err(StoreError::CoordinationDisabled) => {
                        (AuthorityKnowledge::CoordinationDisabled, Vec::new())
                    }
                    Err(error) => return Err(store_failed(error)),
                };
            known_sessions.insert(
                record.session.id.0,
                KnownSession {
                    kernel: record.session.kernel,
                    owner: record.session.owner,
                    status: record.session.status,
                    authority,
                    cleanup_debts,
                },
            );
        }

        let mut providers = Vec::with_capacity(listed_providers.len());
        for listed in listed_providers {
            let view = match listed.view {
                ListedProviderView::Observed(runtimes) => ProviderView::Observed(
                    runtimes
                        .into_iter()
                        .map(|listing| {
                            let known = listing
                                .session
                                .as_deref()
                                .and_then(|session| known_sessions.get(session));
                            let listed_generation = match listing.generation {
                                layer3_adapter::RuntimeGeneration::Legacy => Some(0),
                                layer3_adapter::RuntimeGeneration::Recorded(generation) => {
                                    Some(generation.get())
                                }
                                layer3_adapter::RuntimeGeneration::Malformed => None,
                            };
                            let cleanup_reason = known.and_then(|session| {
                                session
                                    .cleanup_debts
                                    .iter()
                                    .find(|cleanup| {
                                        cleanup.runtime_id == listing.handle.0
                                            && Some(cleanup.generation) == listed_generation
                                    })
                                    .map(|cleanup| cleanup.reason)
                            });
                            let (durable_authority, authority_comparison) = match known {
                                None => (None, RuntimeAuthorityComparison::Unaccounted),
                                Some(KnownSession {
                                    authority: AuthorityKnowledge::CoordinationDisabled,
                                    ..
                                }) => (None, RuntimeAuthorityComparison::CoordinationDisabled),
                                Some(KnownSession {
                                    kernel: durable_kernel,
                                    authority: AuthorityKnowledge::Present(authority),
                                    ..
                                }) => {
                                    let same_runtime = durable_kernel == &listed.kernel
                                        && authority.runtime_id.as_deref()
                                            == Some(listing.handle.0.as_str());
                                    let comparison = match (
                                        authority.generation,
                                        listing.generation,
                                        same_runtime,
                                    ) {
                                        (0, layer3_adapter::RuntimeGeneration::Legacy, true) => {
                                            RuntimeAuthorityComparison::Legacy
                                        }
                                        (
                                            durable,
                                            layer3_adapter::RuntimeGeneration::Recorded(provider),
                                            true,
                                        ) if durable == provider.get() => {
                                            RuntimeAuthorityComparison::Matches
                                        }
                                        _ => RuntimeAuthorityComparison::Mismatch,
                                    };
                                    (Some(authority.clone()), comparison)
                                }
                            };
                            RuntimeEntry {
                                runtime: listing.handle.0,
                                generation: listing.generation.into(),
                                durable_authority,
                                authority_comparison,
                                cleanup_reason,
                                // Which control plane created it, so an
                                // operator reading a shared provider can tell
                                // this deployment's sandboxes from another's
                                // rather than wondering whose a stranger is.
                                created_by: listing.created_by,
                                session: listing.session.map(SessionId),
                                owner: known.map(|session| session.owner.clone()),
                                status: known.map(|session| session.status),
                            }
                        })
                        .collect(),
                ),
                ListedProviderView::Unobservable(error) => ProviderView::Unobservable(error),
            };
            providers.push(ProviderInventory {
                kernel: listed.kernel,
                view,
            });
        }
        // Ordered so two reads of an unchanged fleet render identically; the
        // adapter map has no order of its own.
        providers.sort_by(|a, b| a.kernel.as_str().cmp(b.kernel.as_str()));

        let inventory = RuntimeInventory { providers };
        tracing::info!(
            reader = %caller.subject,
            running = inventory.running().map(|n| n.to_string()).unwrap_or_else(|| "<unknown>".into()),
            fully_observed = inventory.fully_observed(),
            "the runtimes in service were listed"
        );
        Ok(inventory)
    }
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;

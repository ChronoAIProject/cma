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

use std::collections::HashMap;

use crate::authz::{Caller, Capability};
use crate::error::CoreError;
use crate::ids::{KernelId, SessionId, SubjectId};
use crate::service::{store_failed, SessionService};
use crate::session::SessionStatus;

/// One runtime, and what the record knows about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeEntry {
    /// The provider's own identifier for it. Opaque; never parsed.
    pub runtime: String,
    /// Durable generation recorded by the provider, when this runtime has one.
    ///
    /// Absence marks a legacy runtime. It must stay visible as absence so an
    /// operator or reclamation path cannot mistake it for current authority.
    pub generation: Option<u64>,
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
    /// that nobody is entitled to, and the sweep will reclaim it.
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

        // Read before asking the providers. A record that cannot be read means
        // no runtime can be attributed to anybody, and an inventory of
        // anonymous sandboxes would be worse than saying so.
        let records = self.store.scan_all().await.map_err(store_failed)?;
        let owners: HashMap<&str, (&SubjectId, SessionStatus)> = records
            .iter()
            .map(|r| (r.session.id.as_str(), (&r.session.owner, r.session.status)))
            .collect();

        let mut providers = Vec::with_capacity(self.adapters.len());
        for (kernel, adapter) in &self.adapters {
            let view = match adapter.list_runtimes().await {
                Ok(runtimes) => ProviderView::Observed(
                    runtimes
                        .into_iter()
                        .map(|listing| {
                            let known = listing.session.as_deref().and_then(|s| owners.get(s));
                            RuntimeEntry {
                                runtime: listing.handle.0,
                                generation: listing.generation,
                                // Which control plane created it, so an
                                // operator reading a shared provider can tell
                                // this deployment's sandboxes from another's
                                // rather than wondering whose a stranger is.
                                created_by: listing.created_by,
                                session: listing.session.map(SessionId),
                                owner: known.map(|(owner, _)| (*owner).clone()),
                                status: known.map(|(_, status)| *status),
                            }
                        })
                        .collect(),
                ),
                Err(e) => {
                    tracing::warn!(
                        kernel = %kernel,
                        error = %e,
                        "a provider cannot say what it is running; the inventory will say so \
                         rather than report it as running nothing"
                    );
                    ProviderView::Unobservable(e.to_string())
                }
            };
            providers.push(ProviderInventory {
                kernel: kernel.clone(),
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

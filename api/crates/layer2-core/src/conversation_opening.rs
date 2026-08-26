//! Conversation updates received before their live entry can be published.

use std::{collections::VecDeque, sync::Arc};

use layer3_adapter::ConversationUpdate;
use tokio::sync::{mpsc, oneshot};

use crate::coordination::{
    ConversationLink, ConversationOpeningLease, ConversationOpeningLeaseRequest, SessionLease,
};
use crate::store::{SessionStore, StoreError};

/// Identity of one process-local conversation link.
///
/// This is deliberately independent of durable runtime generation. Generation
/// zero names every compatibility runtime, while readoption opens a new link to
/// the same durable runtime generation. A random token names exactly one local
/// receiver in both cases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ConversationLinkId(uuid::Uuid);

impl ConversationLinkId {
    fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

/// Armed ownership of an opening acquisition before its store result is known.
///
/// The owner task resolves the exact request independently of its caller. If
/// the caller disappears before handing this claim to a receiver, the task
/// releases whichever exact claim the store committed before it exits.
#[must_use = "an acquired conversation claim must be installed or revoked"]
pub(crate) struct ConversationOpeningClaim {
    authority: Option<ConversationOpeningLease>,
    cleanup_disarm: Option<oneshot::Sender<()>>,
    cleanup: Option<tokio::task::JoinHandle<()>>,
}

/// A receiver that has not yet been allowed to mutate a live session entry.
///
/// Layer 2 owns this value while the adapter's open future runs. Ordinary
/// updates are drained into `held` so a runtime replaying more than the channel
/// capacity cannot deadlock its own handshake. On success the caller installs
/// the matching link and folds `held` before starting the long-lived folder. On
/// failure, dropping this value discards both the held and still-queued updates;
/// no folder has existed and no ordinary update has reached durable history.
#[must_use = "an opening must be installed or revoked while it owns the receiver"]
pub(crate) struct ConversationOpening {
    link: ConversationLinkId,
    held: VecDeque<ConversationUpdate>,
    updates: Option<mpsc::Receiver<ConversationUpdate>>,
    authority: Option<ConversationOpeningLease>,
    store: Arc<dyn SessionStore>,
    cleanup_disarm: Option<oneshot::Sender<()>>,
    cleanup: Option<tokio::task::JoinHandle<()>>,
}

/// Armed cleanup for a durable link that has not reached folder ownership.
#[must_use = "a publication must be handed to its folder or revoked"]
pub(crate) struct ConversationPublication {
    link: ConversationLinkId,
    authority: ConversationLink,
    cleanup_disarm: Option<oneshot::Sender<()>>,
    cleanup: Option<tokio::task::JoinHandle<()>>,
}

impl ConversationOpening {
    pub(crate) fn channel(
        capacity: usize,
        mut claim: ConversationOpeningClaim,
        store: Arc<dyn SessionStore>,
    ) -> (mpsc::Sender<ConversationUpdate>, Self) {
        let (updates, receiver) = mpsc::channel(capacity);
        (
            updates,
            Self {
                link: ConversationLinkId::new(),
                held: VecDeque::new(),
                updates: Some(receiver),
                authority: claim.authority.take(),
                store,
                cleanup_disarm: claim.cleanup_disarm.take(),
                cleanup: claim.cleanup.take(),
            },
        )
    }

    pub(crate) async fn receive(&mut self) -> Option<ConversationUpdate> {
        self.updates
            .as_mut()
            .expect("an opening owns its receiver until publication")
            .recv()
            .await
    }

    pub(crate) fn hold(&mut self, update: ConversationUpdate) {
        self.held.push_back(update);
    }

    /// A sign-in may be published early only before any ordinary conversation
    /// frame. Publishing it past an already-buffered frame would reorder durable
    /// history; buffering it instead would hide the prompt that unblocks open.
    pub(crate) fn can_publish_sign_in(&self) -> bool {
        self.held.is_empty()
    }

    pub(crate) fn authority(&self) -> &ConversationOpeningLease {
        self.authority
            .as_ref()
            .expect("an opening owns its authority until publication")
    }

    /// Release this exact unpublished claim while the caller owns the local
    /// lifecycle, input, and conversation-order gates.
    pub(crate) async fn revoke(mut self) {
        self.updates.take();
        self.held.clear();
        self.authority.take();
        self.cleanup_disarm.take();
        if let Some(cleanup) = self.cleanup.take() {
            let _ = cleanup.await;
        }
    }

    /// Publish while retained cleanup still owns every cancellation boundary.
    pub(crate) async fn publish(
        mut self,
        session_lease: Option<&SessionLease>,
    ) -> Result<
        (
            ConversationLinkId,
            ConversationLink,
            VecDeque<ConversationUpdate>,
            mpsc::Receiver<ConversationUpdate>,
            ConversationPublication,
        ),
        StoreError,
    > {
        let authority = self
            .authority
            .as_ref()
            .expect("an opening publishes its authority exactly once")
            .clone();
        let mut delay = std::time::Duration::from_millis(5);
        let durable = loop {
            match self
                .store
                .publish_conversation_link(&authority, session_lease)
                .await
            {
                Ok(durable) => break durable,
                Err(StoreError::Unavailable(error)) => {
                    tracing::warn!(
                        session_id = %authority.session_id,
                        %error,
                        "conversation link publication result is unknown; retrying the exact claim"
                    );
                    tokio::time::sleep(delay).await;
                    delay = delay
                        .saturating_mul(2)
                        .min(std::time::Duration::from_secs(5));
                }
                Err(error) => {
                    self.revoke().await;
                    return Err(error);
                }
            }
        };
        self.authority.take();
        let publication = ConversationPublication {
            link: self.link,
            authority: durable.clone(),
            cleanup_disarm: self.cleanup_disarm.take(),
            cleanup: self.cleanup.take(),
        };
        Ok((
            self.link,
            durable,
            std::mem::take(&mut self.held),
            self.updates
                .take()
                .expect("a published opening still owns its receiver"),
            publication,
        ))
    }
}

impl ConversationOpeningClaim {
    /// Start resolving one exact acquisition while cleanup is already armed.
    pub(crate) async fn acquire(
        store: Arc<dyn SessionStore>,
        request: ConversationOpeningLeaseRequest,
        session_lease: Option<SessionLease>,
    ) -> Result<Self, StoreError> {
        let expected_authority = ConversationOpeningLease {
            session_id: request.session_id.clone(),
            holder: request.holder.clone(),
            token: request.token.clone(),
            runtime_id: request.runtime_id.clone(),
            runtime_generation: request.runtime_generation,
            runtime_candidate_token: request.runtime_candidate_token.clone(),
            session_lease_token: session_lease.as_ref().map(|lease| lease.token.clone()),
        };
        let (cleanup_disarm, disarmed) = oneshot::channel();
        let (result_tx, result_rx) = oneshot::channel();
        let cleanup_store = Arc::clone(&store);
        let cleanup = tokio::spawn(async move {
            let (outcome, saw_unknown) =
                acquire_until_resolved(Arc::clone(&cleanup_store), request, session_lease).await;
            match outcome {
                Ok(authority) => {
                    if result_tx.send(Ok(authority.clone())).is_err() || disarmed.await.is_err() {
                        release_until_resolved(cleanup_store, authority).await;
                    }
                }
                Err(error) => {
                    // A definitive retry failure does not prove the first,
                    // unavailable response failed to commit. Clear that exact
                    // deterministic claim before allowing a successor to race.
                    if saw_unknown {
                        release_until_resolved(cleanup_store, expected_authority).await;
                    }
                    let _ = result_tx.send(Err(error));
                }
            }
        });
        let authority = result_rx.await.map_err(|_| {
            StoreError::Unavailable(
                "conversation opening acquisition task stopped before ownership transfer".into(),
            )
        })??;
        Ok(Self {
            authority: Some(authority),
            cleanup_disarm: Some(cleanup_disarm),
            cleanup: Some(cleanup),
        })
    }

    #[cfg(test)]
    pub(crate) fn authority(&self) -> &ConversationOpeningLease {
        self.authority
            .as_ref()
            .expect("an acquired conversation claim owns its authority until transfer")
    }

    #[cfg(test)]
    pub(crate) async fn revoke(mut self) {
        self.authority.take();
        self.cleanup_disarm.take();
        if let Some(cleanup) = self.cleanup.take() {
            let _ = cleanup.await;
        }
    }
}

impl ConversationPublication {
    pub(crate) fn link(&self) -> ConversationLinkId {
        self.link
    }

    pub(crate) fn authority(&self) -> &ConversationLink {
        &self.authority
    }

    /// Transfer cleanup ownership to the folder without a cancellation point.
    pub(crate) fn disarm(mut self) {
        if let Some(disarm) = self.cleanup_disarm.take() {
            let _ = disarm.send(());
        }
        self.cleanup.take();
    }

    /// Retire an exact publication before a folder can own its receiver.
    pub(crate) async fn revoke(mut self) {
        self.cleanup_disarm.take();
        if let Some(cleanup) = self.cleanup.take() {
            let _ = cleanup.await;
        }
    }
}

impl Drop for ConversationPublication {
    fn drop(&mut self) {
        self.cleanup_disarm.take();
        self.cleanup.take();
    }
}

impl Drop for ConversationOpeningClaim {
    fn drop(&mut self) {
        self.authority.take();
        self.cleanup_disarm.take();
        self.cleanup.take();
    }
}

impl Drop for ConversationOpening {
    fn drop(&mut self) {
        self.updates.take();
        self.held.clear();
        self.authority.take();
        self.cleanup_disarm.take();
        self.cleanup.take();
    }
}

async fn release_until_resolved(store: Arc<dyn SessionStore>, authority: ConversationOpeningLease) {
    let mut delay = std::time::Duration::from_millis(5);
    loop {
        match store.release_conversation_opening(&authority).await {
            Ok(()) => return,
            Err(StoreError::Unavailable(error)) => {
                tracing::warn!(
                    session_id = %authority.session_id,
                    %error,
                    "conversation opening release retained for retry"
                );
                tokio::time::sleep(delay).await;
                delay = delay
                    .saturating_mul(2)
                    .min(std::time::Duration::from_secs(5));
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %authority.session_id,
                    %error,
                    "conversation opening release could not be completed"
                );
                return;
            }
        }
    }
}

async fn acquire_until_resolved(
    store: Arc<dyn SessionStore>,
    request: ConversationOpeningLeaseRequest,
    session_lease: Option<SessionLease>,
) -> (Result<ConversationOpeningLease, StoreError>, bool) {
    let mut delay = std::time::Duration::from_millis(5);
    let mut saw_unknown = false;
    loop {
        match store
            .acquire_conversation_opening(&request, session_lease.as_ref())
            .await
        {
            Err(StoreError::Unavailable(error)) => {
                saw_unknown = true;
                tracing::warn!(
                    session_id = %request.session_id,
                    %error,
                    "conversation opening acquisition result is unknown; retrying the exact claim"
                );
                tokio::time::sleep(delay).await;
                delay = delay
                    .saturating_mul(2)
                    .min(std::time::Duration::from_secs(5));
            }
            outcome => return (outcome, saw_unknown),
        }
    }
}

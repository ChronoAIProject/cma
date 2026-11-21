//! Short-lived, single-use grants for browser upgrade handshakes.

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use layer2_core::{Authorization, Caller, StoreError, SubjectId};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

const LIFETIME: Duration = Duration::from_secs(30);

/// Whether a redeemed grant may replace an existing attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentClaim {
    Exclusive,
    Takeover,
}

/// Opaque grant returned to a browser.
#[derive(Debug, Clone)]
pub struct IssuedTicket {
    pub value: String,
    pub expires_at: String,
}

/// Identity and claim established by a successful redemption.
pub struct RedeemedTicket {
    pub caller: Caller,
    pub claim: AttachmentClaim,
}

/// One-way lookup key derived from an opaque grant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TicketId([u8; 32]);

impl TicketId {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Durable representation of a grant. The opaque bearer value is never stored.
#[derive(Clone)]
pub struct StoredTicket {
    pub id: TicketId,
    pub resource: String,
    pub subject: SubjectId,
    pub authorization: Authorization,
    pub claim: AttachmentClaim,
    pub expires_at: SystemTime,
}

#[async_trait::async_trait]
pub trait TicketStore: Send + Sync {
    async fn put(&self, ticket: StoredTicket) -> Result<(), StoreError>;
    async fn take(&self, id: &TicketId) -> Result<Option<StoredTicket>, StoreError>;
}

#[derive(Default)]
pub struct MemoryTicketStore {
    issued: Mutex<std::collections::HashMap<TicketId, StoredTicket>>,
}

#[async_trait::async_trait]
impl TicketStore for MemoryTicketStore {
    async fn put(&self, ticket: StoredTicket) -> Result<(), StoreError> {
        let now = SystemTime::now();
        let mut issued = self.issued.lock().await;
        issued.retain(|_, held| held.expires_at > now);
        issued.insert(ticket.id.clone(), ticket);
        Ok(())
    }

    async fn take(&self, id: &TicketId) -> Result<Option<StoredTicket>, StoreError> {
        Ok(self.issued.lock().await.remove(id))
    }
}

/// Custody for one-time attachment grants.
pub struct AttachmentTickets {
    store: Arc<dyn TicketStore>,
    lifetime: Duration,
}

impl AttachmentTickets {
    pub fn new(store: Arc<dyn TicketStore>) -> Arc<Self> {
        Arc::new(Self {
            store,
            lifetime: LIFETIME,
        })
    }

    pub fn in_memory() -> Arc<Self> {
        Self::new(Arc::new(MemoryTicketStore::default()))
    }

    #[cfg(test)]
    fn with_lifetime(store: Arc<dyn TicketStore>, lifetime: Duration) -> Arc<Self> {
        Arc::new(Self { store, lifetime })
    }

    pub async fn issue(
        &self,
        caller: &Caller,
        resource: &str,
        claim: AttachmentClaim,
    ) -> Result<IssuedTicket, StoreError> {
        let value = uuid::Uuid::new_v4().simple().to_string();
        let expires_at = SystemTime::now() + self.lifetime;
        self.store
            .put(StoredTicket {
                id: identify(&value),
                resource: resource.to_owned(),
                subject: caller.subject.clone(),
                authorization: caller.authorization.clone(),
                claim,
                expires_at,
            })
            .await?;
        Ok(IssuedTicket {
            value,
            expires_at: layer2_core::rfc3339_at(expires_at),
        })
    }

    pub async fn redeem(
        &self,
        value: &str,
        resource: &str,
    ) -> Result<Option<RedeemedTicket>, StoreError> {
        let Some(ticket) = self.store.take(&identify(value)).await? else {
            return Ok(None);
        };
        if ticket.expires_at <= SystemTime::now() || ticket.resource != resource {
            return Ok(None);
        }
        Ok(Some(RedeemedTicket {
            caller: Caller::new(ticket.subject, ticket.authorization),
            claim: ticket.claim,
        }))
    }
}

fn identify(value: &str) -> TicketId {
    TicketId(Sha256::digest(value.as_bytes()).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller() -> Caller {
        Caller::new(SubjectId::new("subject-a"), Authorization::none())
    }

    #[tokio::test]
    async fn a_ticket_is_resource_bound_and_single_use() {
        let tickets = AttachmentTickets::in_memory();
        let issued = tickets
            .issue(&caller(), "resource-a", AttachmentClaim::Exclusive)
            .await
            .unwrap();

        assert!(tickets
            .redeem(&issued.value, "resource-b")
            .await
            .unwrap()
            .is_none());
        assert!(tickets
            .redeem(&issued.value, "resource-a")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn an_expired_ticket_is_refused() {
        let tickets = AttachmentTickets::with_lifetime(
            Arc::new(MemoryTicketStore::default()),
            Duration::ZERO,
        );
        let issued = tickets
            .issue(&caller(), "resource-a", AttachmentClaim::Takeover)
            .await
            .unwrap();
        assert!(tickets
            .redeem(&issued.value, "resource-a")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn a_live_ticket_restores_the_caller_and_claim() {
        let tickets = AttachmentTickets::in_memory();
        let issued = tickets
            .issue(&caller(), "resource-a", AttachmentClaim::Takeover)
            .await
            .unwrap();
        let redeemed = tickets
            .redeem(&issued.value, "resource-a")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(redeemed.caller.subject.as_str(), "subject-a");
        assert_eq!(redeemed.claim, AttachmentClaim::Takeover);
        assert!(tickets
            .redeem(&issued.value, "resource-a")
            .await
            .unwrap()
            .is_none());
    }
}

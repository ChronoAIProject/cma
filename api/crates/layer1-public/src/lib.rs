//! Public-boundary foundations retained while the resource APIs are rebuilt.

pub mod a2a;
pub mod agui;
pub mod auth;
pub mod mcp;
pub mod responses;
pub mod rest;
pub mod tickets;

pub use auth::{AuthenticatedCaller, AuthenticatedSubject, DelegatedCredential};
pub use tickets::{
    AttachmentClaim, AttachmentTickets, IssuedTicket, MemoryTicketStore, RedeemedTicket,
    StoredTicket, TicketId, TicketStore,
};

/// A public-interface operation that remains available during the rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operation {
    Discover,
}

/// Protocol profiles declare only operations they currently implement.
pub trait InterfaceProfile {
    fn interface_id(&self) -> &'static str;
    fn operations(&self) -> &'static [Operation];
}

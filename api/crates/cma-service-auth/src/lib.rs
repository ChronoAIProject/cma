//! Technical OAuth transport shared by independently configured services.
//! Each caller supplies its own client, cookie keys and exclusive DB collections.
pub mod browser;
pub mod mongodb_authority;
mod nyx;
pub mod request_audit;
mod service_key;
pub mod session;
pub mod signing_keys;
pub use nyx::{AuthError, NyxIdAuth, Principal};
pub use service_key::{is_service_key, ServiceKeyAuth, ServiceKeyPrincipal};
#[derive(Clone, Copy, Debug)]
pub enum Service {
    EventGateway,
}
impl Service {
    pub fn slug(self) -> &'static str {
        match self {
            Self::EventGateway => "cma-event-gateway",
        }
    }
    pub fn permission_prefix(self) -> &'static str {
        match self {
            Self::EventGateway => "cma-eg",
        }
    }
}
fn bounded_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

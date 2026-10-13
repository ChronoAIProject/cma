//! Technical OAuth transport shared by independently configured services.
//! Each caller supplies its own client, cookie keys and exclusive DB collections.
pub mod browser;
pub mod mongodb_authority;
mod nyx;
pub use nyx::{AuthError, NyxIdAuth, Principal};
#[derive(Clone, Copy, Debug)]
pub enum Service {
    EventGateway,
    ImageManager,
}
impl Service {
    pub fn slug(self) -> &'static str {
        match self {
            Self::EventGateway => "cma-event-gateway",
            Self::ImageManager => "cma-image-manager",
        }
    }
    pub fn permission_prefix(self) -> &'static str {
        match self {
            Self::EventGateway => "cma-eg",
            Self::ImageManager => "cma-im",
        }
    }
}
fn bounded_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

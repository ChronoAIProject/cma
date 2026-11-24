//! Platform rendering is separate from transport and send authority.
pub mod custom;
mod relay;
mod text;
use crate::tools::Capabilities;
use cmaeg_core::message::{OutboundMessage, RenderingReport};
pub use relay::{LiteralText, RelayText};
pub use text::literal_platform;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPart {
    pub text: String,
}
pub trait Renderer {
    fn render(
        &self,
        message: &OutboundMessage,
        capabilities: &Capabilities,
    ) -> (Vec<RenderedPart>, RenderingReport);
}

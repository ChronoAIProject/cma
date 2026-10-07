//! NyxID relay authentication and per-platform event normalization.
pub mod activity;
pub mod facts;
mod generic;
pub mod jwks;
mod jwt;
pub(crate) mod lark;
pub mod payload;
pub(crate) mod telegram;
mod telegram_managed;
pub mod verify;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerificationError(pub &'static str);
impl std::fmt::Display for VerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for VerificationError {}

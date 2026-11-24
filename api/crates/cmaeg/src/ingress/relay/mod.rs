//! NyxID relay authentication and per-platform event normalization.
pub mod activity;
pub mod attachments;
mod discord;
mod envelope_errors;
pub mod facts;
mod generic;
pub mod jwks;
mod jwt;
pub(crate) mod lark;
mod lark_callback;
mod lark_media;
mod lark_post;
mod lark_resources;
pub mod payload;
#[cfg(test)]
mod seeded_rng;
pub(crate) mod telegram;
mod telegram_managed;
pub(crate) mod telegram_media;
pub(crate) mod telegram_member_joins;
mod verified_media;
pub mod verify;
mod whatsapp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerificationError(pub &'static str);
impl std::fmt::Display for VerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for VerificationError {}

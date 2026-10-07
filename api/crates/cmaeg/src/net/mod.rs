//! Outbound address policy shared by every gateway fetch of a caller-named URL.
//!
//! Webhook delivery and any later fetch of a caller-supplied destination must
//! apply the same SSRF rules: only public HTTPS targets, DNS answers checked
//! on every attempt, and connections pinned to the checked addresses.
pub mod policy;
pub mod resolve;

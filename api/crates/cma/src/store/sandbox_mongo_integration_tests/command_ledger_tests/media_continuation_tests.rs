//! Real Mongo, public binary route and file publication with an attestation peer.
use super::*;

#[path = "media_continuation_ownership_tests.rs"]
mod ownership;
use crate::media_continuations::{MediaContinuations, MediaUploadRecord, MediaUploadStore};
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{Request, StatusCode},
    routing::post,
    Json, Router,
};
use cmaeg_core::{
    activity::Activity,
    media_continuation::{sha256, supplement_text, MediaArtifactClaim, MediaContinuationKind},
};
use layer1_public::{
    rest::binding_media::*, AuthenticatedResourceCaller, AuthenticatedSubject, DelegatedCredential,
};
use layer2_core::sandbox::agent_attachments::*;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::atomic::AtomicBool};
use tower::ServiceExt;

#[path = "media_continuation_tests/ownership_handoff.rs"]
mod ownership_handoff;

include!("media_continuation_tests/fixture.rs");
include!("media_continuation_tests/uploads.rs");
include!("media_continuation_tests/runtime_delivery.rs");
include!("media_continuation_tests/requeue.rs");
include!("media_continuation_tests/retry_authority.rs");

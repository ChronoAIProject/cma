//! OpenAPI-only projections for routes whose handlers live outside the REST schema boundary.

#![allow(
    dead_code,
    reason = "these types and functions exist only for OpenAPI generation"
)]

use utoipa::ToSchema;

use super::super::launch_events::LaunchEventView;
use super::super::trigger_openapi_shapes::{TriggerButtonLabelValue, TriggerThemeValue};
use super::super::ProblemDocument;

#[derive(ToSchema)]
pub(super) struct SignupOptions {
    social_providers: Vec<String>,
    email_signup: bool,
    invite_code_required: bool,
    register_available: bool,
}

#[derive(ToSchema)]
pub(super) struct EmbedSnippets {
    markdown: String,
    html: String,
    javascript: String,
    react: String,
    url: String,
}

#[derive(ToSchema)]
pub(super) struct EmbedDescriptor {
    public_url: String,
    badge_url: String,
    label_text: String,
    javascript_sri: String,
    snippets: EmbedSnippets,
}

#[utoipa::path(
    get,
    path = "/auth/signup-options",
    tag = "discovery",
    responses((status = 200, body = SignupOptions)),
    security(())
)]
pub(super) fn signup_options() {}

#[utoipa::path(
    get,
    path = "/api/v1/triggers/public/{public_id}/badge.svg",
    tag = "triggers",
    params(
        ("public_id" = String, Path),
        ("label" = Option<TriggerButtonLabelValue>, Query),
        ("theme" = Option<TriggerThemeValue>, Query),
        ("placement" = Option<String>, Query),
        ("If-None-Match" = Option<String>, Header)
    ),
    responses(
        (status = 200, body = String, content_type = "image/svg+xml"),
        (status = 304),
        (status = 400, body = ProblemDocument),
        (status = 503, body = ProblemDocument)
    ),
    security(())
)]
pub(super) fn badge() {}

#[utoipa::path(
    get,
    path = "/api/v1/triggers/{id}/embed",
    tag = "trigger placements",
    params(("id" = String, Path), ("placement_id" = String, Query)),
    responses(
        (status = 200, body = EmbedDescriptor),
        (status = 400, body = ProblemDocument),
        (status = 404, body = ProblemDocument),
        (status = 409, body = ProblemDocument),
        (status = 503, body = ProblemDocument)
    )
)]
pub(super) fn embed() {}

#[utoipa::path(
    get,
    path = "/api/v1/agui/launches/{id}/events",
    tag = "trigger launches",
    params(
        ("id" = String, Path),
        ("starting_after" = Option<u64>, Query),
        ("after" = Option<u64>, Query),
        ("limit" = Option<u16>, Query),
        ("follow" = Option<bool>, Query),
        ("Last-Event-ID" = Option<u64>, Header)
    ),
    responses(
        (status = 200, body = LaunchEventView, content_type = "text/event-stream"),
        (status = 400, body = ProblemDocument),
        (status = 404, body = ProblemDocument),
        (status = 503, body = ProblemDocument)
    )
)]
pub(super) fn launch_event_companion() {}

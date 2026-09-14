//! Owner-only descriptor generated from one immutable Trigger revision.

use axum::{
    extract::{Path, RawQuery, Request, State},
    http::{header, HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::trigger_domain::{
    ButtonLabel, PlacementId, PublicTriggerId, Theme, TriggerAvailability, TriggerId,
    TriggerRevision,
};
use serde::{Deserialize, Serialize};
use url::Url;

use super::{is_embed_identifier, problem_document, problem_response, TriggerAssetsState};
use crate::AuthenticatedCaller;

/// Subresource integrity for the exact bytes in `web/public/embed/v1.js`.
pub const EMBED_V1_JAVASCRIPT_SRI: &str =
    "sha384-K56ODoEc3ZTiX/9Xfe3xk5PEm0Q+bQw5brGMyIfL3hGTeREM9Xf7zvR/7nn1XGuI";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmbedQuery {
    placement_id: String,
}

/// Every supported representation of one live Trigger placement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedDescriptor {
    pub public_url: String,
    pub badge_url: String,
    pub label_text: String,
    pub javascript_sri: String,
    pub content_security_policy: EmbedContentSecurityPolicy,
    pub snippets: EmbedSnippets,
}

/// Copy-ready source directives for the configured, validated public origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedContentSecurityPolicy {
    pub img_src: String,
    pub script_src: String,
}

/// Copy-ready snippets derived from the same canonical URLs and presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedSnippets {
    pub markdown: String,
    pub html: String,
    pub javascript: String,
    pub react: String,
    pub url: String,
}

pub(super) async fn read(
    State(state): State<TriggerAssetsState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(trigger_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let query = match raw_query
        .as_deref()
        .map(serde_urlencoded::from_str::<EmbedQuery>)
        .transpose()
    {
        Ok(Some(query)) => query,
        Ok(None) | Err(_) => {
            return descriptor_problem(
                StatusCode::BAD_REQUEST,
                "invalid-embed-query",
                "The embed request must name exactly one placement_id.",
                "invalid_embed_query",
            )
        }
    };
    if !is_domain_id(&trigger_id, "trg_") || !is_domain_id(&query.placement_id, "plc_") {
        return descriptor_not_found();
    }
    let trigger_id = TriggerId(trigger_id);
    let placement_id = PlacementId(query.placement_id);
    let trigger = match state.triggers.trigger(&trigger_id, &caller.subject).await {
        Ok(Some(trigger)) => trigger,
        Ok(None) => return descriptor_not_found(),
        Err(_) => return descriptor_store_unavailable(),
    };
    if trigger.availability != TriggerAvailability::Published {
        return descriptor_problem(
            StatusCode::CONFLICT,
            "trigger-not-published",
            "An embed descriptor is available only for a published Trigger.",
            "trigger_not_published",
        );
    }
    let Some(public_id) = trigger.public_id.as_ref() else {
        return descriptor_unavailable_state();
    };
    let Some(revision) = trigger.latest_revision() else {
        return descriptor_unavailable_state();
    };
    let placement = match state
        .triggers
        .placement(&trigger_id, &caller.subject, &placement_id)
        .await
    {
        Ok(Some(placement)) if placement.retired_at.is_none() => placement,
        Ok(Some(_)) | Ok(None) => return descriptor_not_found(),
        Err(_) => return descriptor_store_unavailable(),
    };
    let descriptor = build_descriptor(state.public_origin(), public_id, revision, &placement.id);
    Json(descriptor).into_response()
}

fn is_domain_id(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix) && value.len() > prefix.len() && is_embed_identifier(value)
}

fn descriptor_not_found() -> Response {
    descriptor_problem(
        StatusCode::NOT_FOUND,
        "trigger-embed-not-found",
        "No published Trigger placement matches this request.",
        "trigger_embed_not_found",
    )
}

fn descriptor_unavailable_state() -> Response {
    descriptor_problem(
        StatusCode::CONFLICT,
        "trigger-not-published",
        "The Trigger has no complete published revision.",
        "trigger_not_published",
    )
}

fn descriptor_store_unavailable() -> Response {
    descriptor_problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "trigger-store-unavailable",
        "The Trigger store could not be read.",
        "trigger_store_unavailable",
    )
}

fn descriptor_problem(status: StatusCode, slug: &str, detail: &str, code: &str) -> Response {
    problem_response(status, problem_document(status, slug, detail, code))
}

pub(super) async fn private_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    response
}

pub(super) fn build_descriptor(
    public_origin: &Url,
    public_id: &PublicTriggerId,
    revision: &TriggerRevision,
    placement_id: &PlacementId,
) -> EmbedDescriptor {
    let presentation = &revision.content.presentation;
    let label = label_key(presentation.button_label);
    let label_text = label_text(presentation.button_label);
    let theme = theme_key(presentation.theme);
    let public_url = path_url(
        public_origin,
        &[
            "api",
            "v1",
            "triggers",
            "public",
            public_id.as_str(),
            "open",
            placement_id.as_str(),
        ],
    )
    .to_string();
    let mut badge_url = path_url(
        public_origin,
        &[
            "api",
            "v1",
            "triggers",
            "public",
            public_id.as_str(),
            "badge.svg",
        ],
    );
    badge_url
        .query_pairs_mut()
        .append_pair("label", label)
        .append_pair("theme", theme)
        .append_pair("placement", placement_id.as_str());
    let badge_url = badge_url.to_string();
    let javascript_url = path_url(public_origin, &["embed", "v1.js"]).to_string();
    let origin = public_origin.origin().ascii_serialization();

    let markdown = format!("[![{label_text}]({badge_url})]({public_url})");
    let html = format!(
        "<a href=\"{}\"><img src=\"{}\" alt=\"{}\"></a>",
        escape_html_attribute(&public_url),
        escape_html_attribute(&badge_url),
        escape_html_attribute(label_text),
    );
    let javascript = format!(
        "<script src=\"{}\" integrity=\"{}\" crossorigin=\"anonymous\"></script>\n<cma-trigger trigger=\"{}\" placement=\"{}\" label=\"{}\" theme=\"{}\" origin=\"{}\"></cma-trigger>",
        escape_html_attribute(&javascript_url),
        EMBED_V1_JAVASCRIPT_SRI,
        escape_html_attribute(public_id.as_str()),
        escape_html_attribute(placement_id.as_str()),
        label,
        theme,
        escape_html_attribute(&origin),
    );
    let react = format!(
        "import {{ CmaTriggerButton }} from '@chronoai/cma-sdk/react';\n\n<CmaTriggerButton triggerId=\"{}\" placementId=\"{}\" label=\"{}\" theme=\"{}\" baseUrl=\"{}\" />",
        escape_jsx_attribute(public_id.as_str()),
        escape_jsx_attribute(placement_id.as_str()),
        label,
        theme,
        escape_jsx_attribute(&origin),
    );
    let snippets = EmbedSnippets {
        markdown,
        html,
        javascript,
        react,
        url: public_url.clone(),
    };
    EmbedDescriptor {
        public_url,
        badge_url,
        label_text: label_text.into(),
        javascript_sri: EMBED_V1_JAVASCRIPT_SRI.into(),
        content_security_policy: EmbedContentSecurityPolicy {
            img_src: format!("img-src {origin}"),
            script_src: format!("script-src {origin}"),
        },
        snippets,
    }
}

fn path_url(origin: &Url, segments: &[&str]) -> Url {
    let mut url = origin.clone();
    url.path_segments_mut()
        .expect("a validated HTTP(S) origin is a hierarchical URL")
        .clear()
        .extend(segments);
    url
}

fn escape_html_attribute(value: &str) -> String {
    escape_attribute(value, true)
}

fn escape_jsx_attribute(value: &str) -> String {
    escape_attribute(value, false)
}

fn escape_attribute(value: &str, escape_ampersand: bool) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' if escape_ampersand => escaped.push_str("&amp;"),
            '"' => escaped.push_str("&quot;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn label_key(label: ButtonLabel) -> &'static str {
    match label {
        ButtonLabel::OpenInCma => "open_in_cma",
        ButtonLabel::ChatWithThis => "chat_with_this",
        ButtonLabel::RunWithCma => "run_with_cma",
        ButtonLabel::TryIt => "try_it",
    }
}

fn label_text(label: ButtonLabel) -> &'static str {
    match label {
        ButtonLabel::OpenInCma => "Open in CMA",
        ButtonLabel::ChatWithThis => "Chat with this",
        ButtonLabel::RunWithCma => "Run with CMA",
        ButtonLabel::TryIt => "Try it",
    }
}

fn theme_key(theme: Theme) -> &'static str {
    match theme {
        Theme::Auto => "auto",
        Theme::Light => "light",
        Theme::Dark => "dark",
    }
}

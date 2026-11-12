//! Anonymous, proxy-safe SVG projection of one versioned Trigger.

use axum::{
    body::Body,
    extract::{Path, RawQuery, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use layer2_core::trigger_domain::{
    ButtonLabel, PlacementId, PublicTriggerId, Theme, TriggerAvailability,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{
    badge_mark::CMA_MARK, is_embed_identifier, problem_document, problem_response,
    TriggerAssetsState,
};

impl TriggerAssetsState {
    pub fn with_account_standing(
        mut self,
        standing: std::sync::Arc<dyn layer2_core::account_deletion::AccountStanding>,
    ) -> Self {
        self.standing = standing;
        self
    }
}

const KNOWN_CACHE_CONTROL: &str = "public, max-age=300, stale-while-revalidate=3600";
const UNKNOWN_CACHE_CONTROL: &str = "public, max-age=60";
const SVG_CONTENT_TYPE: &str = "image/svg+xml; charset=utf-8";
const SVG_CONTENT_SECURITY_POLICY: &str = "default-src 'none'; style-src 'unsafe-inline'";
const MAX_BADGE_BYTES: usize = 4_096;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BadgeQuery {
    #[serde(default)]
    label: Option<ButtonLabel>,
    #[serde(default)]
    theme: Option<Theme>,
    #[serde(default)]
    placement: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BadgeLabel {
    Trigger(ButtonLabel),
    Cma,
}

impl BadgeLabel {
    fn key(self) -> &'static str {
        match self {
            Self::Trigger(ButtonLabel::OpenInCma) => "open_in_cma",
            Self::Trigger(ButtonLabel::ChatWithThis) => "chat_with_this",
            Self::Trigger(ButtonLabel::RunWithCma) => "run_with_cma",
            Self::Trigger(ButtonLabel::TryIt) => "try_it",
            Self::Cma => "cma",
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::Trigger(ButtonLabel::OpenInCma) => "Open in CMA",
            Self::Trigger(ButtonLabel::ChatWithThis) => "Chat with this",
            Self::Trigger(ButtonLabel::RunWithCma) => "Run with CMA",
            Self::Trigger(ButtonLabel::TryIt) => "Try it",
            Self::Cma => "CMA",
        }
    }

    fn text_length(self) -> u16 {
        match self {
            Self::Trigger(ButtonLabel::OpenInCma) => 78,
            Self::Trigger(ButtonLabel::ChatWithThis) => 84,
            Self::Trigger(ButtonLabel::RunWithCma) => 82,
            Self::Trigger(ButtonLabel::TryIt) => 31,
            Self::Cma => 28,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BadgeAvailability {
    Available,
    Unavailable,
    Generic,
}

impl BadgeAvailability {
    fn key(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unavailable => "unavailable",
            Self::Generic => "generic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BadgeSpec {
    pub(super) label: BadgeLabel,
    pub(super) theme: Theme,
    pub(super) availability: BadgeAvailability,
}

impl BadgeSpec {
    fn title(self) -> &'static str {
        match self.availability {
            BadgeAvailability::Available => self.label.text(),
            BadgeAvailability::Unavailable => "Unavailable",
            BadgeAvailability::Generic => "CMA",
        }
    }
}

pub(super) async fn read(
    State(state): State<TriggerAssetsState>,
    Path(public_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let query = match raw_query.as_deref() {
        Some(raw) => match serde_urlencoded::from_str::<BadgeQuery>(raw) {
            Ok(query) => query,
            Err(_) => return badge_problem(StatusCode::BAD_REQUEST, "invalid-badge-query"),
        },
        None => BadgeQuery::default(),
    };
    if query
        .placement
        .as_deref()
        .is_some_and(|placement| !is_embed_identifier(placement))
    {
        return badge_problem(StatusCode::BAD_REQUEST, "invalid-badge-query");
    }

    let requested_theme = query.theme.unwrap_or(Theme::Auto);
    if !is_embed_identifier(&public_id) || !public_id.starts_with("pub_") {
        return generic_badge(requested_theme, &headers);
    }
    let public_id = PublicTriggerId(public_id);
    let published = match state.triggers.trigger_by_public_id(&public_id).await {
        Ok(Some(published)) => published,
        Ok(None) => return generic_badge(requested_theme, &headers),
        Err(_) => return badge_problem(StatusCode::SERVICE_UNAVAILABLE, "badge-unavailable"),
    };

    let label = BadgeLabel::Trigger(
        query
            .label
            .unwrap_or(published.revision.content.presentation.button_label),
    );
    let theme = query
        .theme
        .unwrap_or(published.revision.content.presentation.theme);
    let erasing = match state.standing.erasing(&published.owner).await {
        Ok(erasing) => erasing,
        Err(_) => return badge_problem(StatusCode::SERVICE_UNAVAILABLE, "badge-unavailable"),
    };
    let mut available = published.availability == TriggerAvailability::Published && !erasing;
    if available {
        if let Some(placement_id) = query.placement {
            let placement = state
                .triggers
                .placement(
                    &published.trigger_id,
                    &published.owner,
                    &PlacementId(placement_id),
                )
                .await;
            available = match placement {
                Ok(Some(placement)) => placement.retired_at.is_none(),
                Ok(None) => false,
                Err(_) => {
                    return badge_problem(StatusCode::SERVICE_UNAVAILABLE, "badge-unavailable")
                }
            };
        }
    }
    let availability = if available {
        BadgeAvailability::Available
    } else {
        BadgeAvailability::Unavailable
    };
    badge_response(
        BadgeSpec {
            label,
            theme,
            availability,
        },
        &published.revision.digest,
        true,
        &headers,
    )
}

fn generic_badge(theme: Theme, headers: &HeaderMap) -> Response {
    badge_response(
        BadgeSpec {
            label: BadgeLabel::Cma,
            theme,
            availability: BadgeAvailability::Generic,
        },
        "unknown",
        false,
        headers,
    )
}

fn badge_response(
    spec: BadgeSpec,
    revision_digest: &str,
    known: bool,
    request_headers: &HeaderMap,
) -> Response {
    let etag = badge_etag(revision_digest, spec);
    let not_modified = if_none_match(request_headers, &etag);
    let body = if not_modified {
        Body::empty()
    } else {
        Body::from(render_badge(spec))
    };
    let mut response = Response::new(body);
    *response.status_mut() = if not_modified {
        StatusCode::NOT_MODIFIED
    } else {
        StatusCode::OK
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(SVG_CONTENT_TYPE),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if known {
            KNOWN_CACHE_CONTROL
        } else {
            UNKNOWN_CACHE_CONTROL
        }),
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).expect("a quoted SHA-256 is a valid ETag"),
    );
    insert_security_headers(headers, SVG_CONTENT_SECURITY_POLICY);
    response
}

fn badge_problem(status: StatusCode, code: &'static str) -> Response {
    let (slug, detail) = match status {
        StatusCode::BAD_REQUEST => (
            "invalid-badge-query",
            "The badge query contains an unsupported value.",
        ),
        _ => (
            "badge-unavailable",
            "The badge could not be read from the Trigger store.",
        ),
    };
    let mut response = problem_response(status, problem_document(status, slug, detail, code));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    insert_security_headers(response.headers_mut(), "default-src 'none'");
    response
}

fn insert_security_headers(headers: &mut HeaderMap, content_security_policy: &'static str) {
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(content_security_policy),
    );
}

pub(super) fn badge_etag(revision_digest: &str, spec: BadgeSpec) -> String {
    let mut digest = Sha256::new();
    for component in [
        revision_digest,
        spec.label.key(),
        theme_key(spec.theme),
        spec.availability.key(),
    ] {
        digest.update(component.as_bytes());
        digest.update([0]);
    }
    format!("\"{:x}\"", digest.finalize())
}

fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    headers.get_all(header::IF_NONE_MATCH).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
            })
        })
    })
}

fn theme_key(theme: Theme) -> &'static str {
    match theme {
        Theme::Auto => "auto",
        Theme::Light => "light",
        Theme::Dark => "dark",
    }
}

pub(super) fn render_badge(spec: BadgeSpec) -> String {
    let text_length = spec.label.text_length();
    let width = text_length + 48;
    let style = palette_style(spec.theme, spec.availability);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="32" viewBox="0 0 {width} 32" role="img" aria-labelledby="cma-title">
<title id="cma-title">{title}</title>
<style>{style}</style>
<rect x="0.5" y="0.5" width="{inner_width}" height="31" rx="6" fill="var(--badge-bg)" stroke="var(--badge-border)"/>
<g color="var(--badge-mark)" transform="translate(2 4) scale(0.24)">
{CMA_MARK}
</g>
<text x="38" y="21" fill="var(--badge-text)" font-family="-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif" font-size="13" font-weight="600" textLength="{text_length}" lengthAdjust="spacingAndGlyphs">{label}</text>
</svg>
"#,
        inner_width = width - 1,
        title = spec.title(),
        label = spec.label.text(),
    );
    debug_assert!(svg.len() <= MAX_BADGE_BYTES);
    svg
}

fn palette_style(theme: Theme, availability: BadgeAvailability) -> &'static str {
    let unavailable = availability == BadgeAvailability::Unavailable;
    match (theme, unavailable) {
        (Theme::Auto, false) => {
            ":root{--badge-bg:#ffffff;--badge-border:#d0d7de;--badge-text:#1f2328;--badge-mark:#0a7a55}@media (prefers-color-scheme:dark){:root{--badge-bg:#0d1117;--badge-border:#30363d;--badge-text:#f0f6fc;--badge-mark:#4fd1a1}}"
        }
        (Theme::Light, false) => {
            ":root{--badge-bg:#ffffff;--badge-border:#d0d7de;--badge-text:#1f2328;--badge-mark:#0a7a55}"
        }
        (Theme::Dark, false) => {
            ":root{--badge-bg:#0d1117;--badge-border:#30363d;--badge-text:#f0f6fc;--badge-mark:#4fd1a1}"
        }
        (Theme::Auto, true) => {
            ":root{--badge-bg:#f6f8fa;--badge-border:#d8dee4;--badge-text:#6e7781;--badge-mark:#8c959f}@media (prefers-color-scheme:dark){:root{--badge-bg:#21262d;--badge-border:#30363d;--badge-text:#8b949e;--badge-mark:#6e7681}}"
        }
        (Theme::Light, true) => {
            ":root{--badge-bg:#f6f8fa;--badge-border:#d8dee4;--badge-text:#6e7781;--badge-mark:#8c959f}"
        }
        (Theme::Dark, true) => {
            ":root{--badge-bg:#21262d;--badge-border:#30363d;--badge-text:#8b949e;--badge-mark:#6e7681}"
        }
    }
}

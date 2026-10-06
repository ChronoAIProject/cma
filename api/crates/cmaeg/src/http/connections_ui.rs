//! Gateway-owned connection setup. Handlers authenticate the browser session,
//! validate Origin and CSRF, and resolve the operation before constructing this
//! view. No authentication or platform credential is accepted by the page model.
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};

const STYLE: &str = include_str!("connections_ui.css");

pub(crate) struct ConnectionSetupPage {
    pub owner_label: String,
    pub platform_label: String,
    pub state: ConnectionSetupState,
}

pub(crate) enum ConnectionSetupState {
    Choose(ConnectionChoices),
    AwaitingPlatform {
        message: String,
        action: Option<PageLink>,
        refresh_path: String,
    },
    Complete {
        connection_label: String,
        action: Option<PageLink>,
    },
    Expired {
        restart: Option<PageLink>,
    },
}

pub(crate) struct ConnectionChoices {
    /// A server-controlled, same-origin POST path; never an upstream API URL.
    pub form_action: String,
    pub csrf_token: String,
    pub selected: ConnectionMethod,
    pub managers: Vec<ManagerChoice>,
    pub management: Option<ManagementChoice>,
    pub registration_help: Option<PageLink>,
    /// A safe user-facing error. Handlers must not copy upstream error bodies.
    pub error: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum ConnectionMethod {
    Existing,
    Create,
}

pub(crate) struct ManagerChoice {
    /// An opaque gateway connection reference, not a platform token or bot ID.
    pub connection_ref: String,
    pub label: String,
}

pub(crate) struct ManagementChoice {
    pub instructions: String,
    pub enable_link: PageLink,
}

pub(crate) struct PageLink {
    pub label: String,
    pub href: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RenderError {
    InvalidAction,
    InvalidLink,
    MissingCsrf,
}

pub(crate) fn render(page: &ConnectionSetupPage) -> Result<Response, RenderError> {
    let body = render_document(page)?;
    let status = if matches!(page.state, ConnectionSetupState::Expired { .. }) {
        StatusCode::GONE
    } else {
        StatusCode::OK
    };
    let mut response = (status, Html(body)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        // Preserve a same-origin Origin header on form POSTs while withholding
        // the referrer when opening the external platform.
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    let style_hash = STANDARD.encode(Sha256::digest(STYLE.as_bytes()));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_str(&format!(
            "default-src 'none'; style-src 'sha256-{style_hash}'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
        ))
        .expect("base64 style digest is a valid header"),
    );
    Ok(response)
}

fn render_document(page: &ConnectionSetupPage) -> Result<String, RenderError> {
    let (heading, lead, content) = match &page.state {
        ConnectionSetupState::Choose(choices) => (
            "Connect your bot",
            "Choose an existing bot or create a new one. Your agent setup will continue when the connection is ready.",
            render_choices(choices, &page.platform_label)?,
        ),
        ConnectionSetupState::AwaitingPlatform { message, action, refresh_path } => {
            if !same_origin_path(refresh_path) {
                return Err(RenderError::InvalidAction);
            }
            let action = optional_link(action, "button")?;
            (
                "Finish connecting your bot",
                "Complete the platform step, then check the connection here.",
                format!(
                    "<section class=\"status-card\" aria-labelledby=\"status-heading\"><span class=\"status-label\">Waiting for you</span><h2 id=\"status-heading\">One more step</h2><p>{}</p><div class=\"actions\">{action}<a class=\"button secondary\" href=\"{}\">Check connection</a></div></section>",
                    escape(message), escape(refresh_path),
                ),
            )
        }
        ConnectionSetupState::Complete { connection_label, action } => (
            "Your bot is connected",
            "The connection is ready. Return to your conversation to continue setting up your agent.",
            format!(
                "<section class=\"status-card\" aria-labelledby=\"status-heading\"><span class=\"status-label complete\">Connected</span><h2 id=\"status-heading\">{}</h2><p>Your bot connection has been saved for the account shown above.</p><div class=\"actions\">{}</div></section>",
                escape(connection_label), optional_link(action, "button")?,
            ),
        ),
        ConnectionSetupState::Expired { restart } => (
            "This setup link has expired",
            "Return to the assistant where you started and request a new connection link.",
            format!("<div class=\"actions\">{}</div>", optional_link(restart, "button")?),
        ),
    };
    Ok(format!(
        r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="robots" content="noindex,nofollow"><title>{heading} · Agent Event Gateway</title><style>{STYLE}</style></head>
<body><a class="skip" href="#content">Skip to content</a><header><a class="brand" href="/docs">Agent Event Gateway</a><span>Connect a bot</span></header>
<main id="content"><div class="context"><span class="platform">{}</span><span>Connecting for <strong>{}</strong></span></div><h1>{heading}</h1><p class="lead">{lead}</p>{content}<footer>Connections are managed by Agent Event Gateway.</footer></main></body></html>"##,
        escape(&page.platform_label),
        escape(&page.owner_label),
    ))
}

fn render_choices(choices: &ConnectionChoices, platform: &str) -> Result<String, RenderError> {
    if !same_origin_path(&choices.form_action) {
        return Err(RenderError::InvalidAction);
    }
    if choices.csrf_token.trim().is_empty() {
        return Err(RenderError::MissingCsrf);
    }
    let existing_checked = if matches!(choices.selected, ConnectionMethod::Existing) {
        " checked"
    } else {
        ""
    };
    let create_checked = if matches!(choices.selected, ConnectionMethod::Create) {
        " checked"
    } else {
        ""
    };
    let error = choices
        .error
        .as_ref()
        .map(|text| format!("<div class=\"error\" role=\"alert\">{}</div>", escape(text)))
        .unwrap_or_default();
    let help = optional_link(&choices.registration_help, "text-link")?;
    let action = escape(&choices.form_action);
    let csrf = escape(&choices.csrf_token);
    let management = match &choices.management {
        Some(option) => format!(
            "<div class=\"capability\"><label class=\"checkbox\"><input type=\"checkbox\" name=\"request_management\" value=\"true\"><span>Also use this bot to help create other bots</span></label><p>This adds a capability. Your bot can still have normal conversations.</p><p>{}</p>{}</div>",
            escape(&option.instructions), render_link(&option.enable_link, "text-link")?,
        ),
        None => String::new(),
    };
    let create_form = if choices.managers.is_empty() {
        "<div class=\"notice\"><h3>No creation bot is available yet</h3><p>Connect an existing bot with management enabled, or ask the person who provided this setup link to make a creation bot available.</p><label class=\"text-link switch\" for=\"method-existing\">Use an existing bot</label></div>".to_owned()
    } else {
        let mut managers = String::new();
        for manager in &choices.managers {
            managers.push_str(&format!(
                "<option value=\"{}\">{}</option>",
                escape(&manager.connection_ref),
                escape(&manager.label)
            ));
        }
        format!(
            r#"<form method="post" action="{action}" autocomplete="off"><input type="hidden" name="csrf_token" value="{csrf}"><input type="hidden" name="intent" value="create_managed">
<label class="field" for="manager">Creation bot<select id="manager" name="manager_connection_ref" required>{managers}</select><span class="hint">This bot will help create and manage your new bot.</span></label>
<label class="field" for="suggested-name">Bot name <span class="optional">optional</span><input id="suggested-name" name="suggested_name" maxlength="64" placeholder="Paper assistant"></label>
<label class="field" for="suggested-username">Bot username <span class="optional">optional</span><input id="suggested-username" name="suggested_username" maxlength="32" autocapitalize="none" spellcheck="false" placeholder="paper_assistant_bot"><span class="hint">You can review and change these suggestions before creating the bot.</span></label>
<p class="hint">You will confirm creation in {}. Nothing is created until you approve it there.</p>{management}<button type="submit">Continue to {}</button></form>"#,
            escape(platform),
            escape(platform),
        )
    };
    Ok(format!(
        r#"{error}<div class="connection-choices">
<fieldset class="methods"><legend>How would you like to connect?</legend><div class="method-options">
<label class="method-card"><input id="method-existing" type="radio" name="connection-method" value="existing"{existing_checked}><span><strong>Use an existing bot</strong><span>Connect a bot you already own with its token.</span></span></label>
<label class="method-card"><input id="method-create" type="radio" name="connection-method" value="create"{create_checked}><span><strong>Create a new bot</strong><span>Choose a creation bot and confirm on the platform.</span></span></label></div></fieldset>
<section class="panel existing-panel" aria-labelledby="existing-heading"><h2 id="existing-heading">Connect an existing bot</h2>
<form method="post" action="{action}" autocomplete="off"><input type="hidden" name="csrf_token" value="{csrf}"><input type="hidden" name="intent" value="connect_existing">
<label class="field" for="connection-label">Connection name <span class="optional">optional</span><input id="connection-label" name="label" maxlength="128" placeholder="My assistant"></label>
<label class="field" for="bot-token">Bot token<input id="bot-token" type="password" name="bot_token" required maxlength="512" autocomplete="off" autocapitalize="none" spellcheck="false" aria-describedby="token-help"></label><p id="token-help" class="hint">Use the token supplied by your bot platform. It is sent directly to the gateway when you connect.</p>{help}{management}<button type="submit">Connect bot</button></form></section>
<section class="panel create-panel" aria-labelledby="create-heading"><h2 id="create-heading">Create a new bot</h2>{create_form}</section></div>"#,
    ))
}

fn optional_link(link: &Option<PageLink>, class: &str) -> Result<String, RenderError> {
    link.as_ref()
        .map(|link| render_link(link, class))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn render_link(link: &PageLink, class: &str) -> Result<String, RenderError> {
    if !allowed_link(&link.href) {
        return Err(RenderError::InvalidLink);
    }
    Ok(format!(
        "<a class=\"{class}\" href=\"{}\" rel=\"noreferrer noopener\">{}</a>",
        escape(&link.href),
        escape(&link.label)
    ))
}

fn same_origin_path(value: &str) -> bool {
    value.starts_with('/')
        && !value.starts_with("//")
        && !value.contains(['\\', '?', '#'])
        && !value
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
}

fn allowed_link(value: &str) -> bool {
    if same_origin_path(value) {
        return true;
    }
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control())
        || value.contains('\\')
    {
        return false;
    }
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && matches!(url.host_str(), Some("t.me" | "core.telegram.org"))
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests;

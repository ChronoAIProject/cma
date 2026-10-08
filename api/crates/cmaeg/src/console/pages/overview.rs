//! The Overview tab: identity, endpoints, settings, the record-size meter,
//! which secrets are set (never their values) and the flag banners.
use cmaeg_core::{ids::RecordId, profile::NyxIdIdentityKind};

use super::{banners, channel::path, escape, facts};
use crate::console::{
    detail::ChannelDetail, directory::ChannelView, source_changes::SourceAvailability,
};

/// The route template of the paste-link form.
pub(crate) const LINK_PATH: &str = "/console/channels/{record_id}/link";
/// The longest channel id the form accepts.
const MAX_CHANNEL_ID: usize = 1024;

/// About the definition size a sealed CMA record can hold.
pub(crate) const RECORD_BYTES_SHOWN: u32 = 48 * 1024;

pub(crate) struct OverviewView<'a> {
    pub view: &'a ChannelView,
    pub public_url: &'a str,
    pub sources: SourceAvailability,
    pub multi_consumer: bool,
    /// Markup shown where the channel id is unknown.
    pub unknown_id: &'a str,
}

pub(crate) fn render(overview: &OverviewView<'_>) -> String {
    let view = overview.view;
    let summary = &view.summary;
    let mut html = banners::routing(
        summary.routed,
        summary.default_receives_all == Some(true),
        overview.multi_consumer,
    );
    html.push_str(&banners::sources(overview.sources));
    for warning in &summary.warnings {
        html.push_str(&format!(
            "<p class=\"notice error\" role=\"status\">Warning: {}</p>",
            escape(&warning.replace('_', " "))
        ));
    }
    let channel_id = view.channel_id.as_ref().map_or_else(
        || overview.unknown_id.to_owned(),
        |id| format!("<code>{}</code>", escape(id.as_str())),
    );
    html.push_str(&facts(&[
        ("Name", escape(&summary.name)),
        (
            "Record id",
            format!("<code>{}</code>", escape(view.record.as_str())),
        ),
        ("Channel id", channel_id),
        (
            "Owner kind",
            summary.owner_kind.map_or("—", owner_kind).to_owned(),
        ),
        ("Version", view.version.to_string()),
        ("Created", escape(&summary.created_at)),
    ]));
    if let Some(id) = &view.channel_id {
        html.push_str(&endpoints(overview, id.as_str()));
    }
    html.push_str(&format!(
        "<h2>Record size</h2><p><meter min=\"0\" max=\"{RECORD_BYTES_SHOWN}\" value=\"{}\"></meter> {:.1} KiB of about {} KiB</p>",
        summary.record_bytes.min(RECORD_BYTES_SHOWN),
        f64::from(summary.record_bytes) / 1024.0,
        RECORD_BYTES_SHOWN / 1024,
    ));
    match &summary.detail {
        Some(detail) => html.push_str(&settings(detail)),
        None => html.push_str(
            "<p class=\"empty\">This channel's definition cannot be read, so its settings are not shown.</p>",
        ),
    }
    html
}

/// Where the channel id would be: a note and the form that links a pasted
/// id after checking it opens to this record. A refusal's message is fixed
/// console text; the pasted value is never echoed.
pub(crate) fn paste_form(record: &RecordId, csrf: &str, error: Option<&str>) -> String {
    let error = error.map_or_else(String::new, |message| {
        format!(
            "<p class=\"notice error\" role=\"alert\">{}</p>",
            escape(message)
        )
    });
    format!(
        "<span class=\"empty\">Not observed since restart; paste to link.</span>{error}<form class=\"filter\" method=\"post\" action=\"{}\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><label for=\"channel_id\">Channel id</label><input id=\"channel_id\" name=\"channel_id\" maxlength=\"{MAX_CHANNEL_ID}\" autocomplete=\"off\" required><button type=\"submit\" class=\"secondary\">Link</button></form>",
        escape(&format!("{}/link", path(record))),
        escape(csrf),
    )
}

/// The addresses senders use, built only from a channel id the console has
/// seen; never a re-sealed one.
fn endpoints(overview: &OverviewView<'_>, id: &str) -> String {
    let base = overview.public_url;
    let mut items = vec![
        (
            "NyxID callback",
            format!(
                "<code>{}</code>",
                escape(&format!("{base}/v1/callbacks/{id}"))
            ),
        ),
        (
            "Custom events",
            format!(
                "<code>{}</code>",
                escape(&format!("{base}/v1/channels/{id}/events"))
            ),
        ),
    ];
    let telegram = overview.view.summary.detail.as_ref().is_some_and(|detail| {
        detail
            .sources
            .iter()
            .any(|source| source.kind == "telegram")
    });
    if telegram {
        items.push((
            "Telegram webhook",
            format!(
                "<code>{}</code>",
                escape(&format!("{base}/v1/telegram/{id}"))
            ),
        ));
    }
    format!("<h2>Endpoints</h2>{}", facts(&items))
}

fn settings(detail: &ChannelDetail) -> String {
    let reply = &detail.reply;
    let sinks = if reply.sinks.is_empty() {
        "none".to_owned()
    } else {
        escape(&reply.sinks.join(", "))
    };
    let secrets = &detail.secrets;
    let set = |value: bool| if value { "set" } else { "not set" }.to_owned();
    let hmac = if secrets.hmac_key_ids.is_empty() {
        "none".to_owned()
    } else {
        format!("set for {}", escape(&secrets.hmac_key_ids.join(", ")))
    };
    let mut html = String::from("<h2>Settings</h2>");
    let custom_key = match (detail.custom_key, detail.custom_key_inert) {
        (None, _) => "same as the partition key".to_owned(),
        (Some(key), false) => key.to_owned(),
        (Some(key), true) => format!("{key} (inert: no custom source is attached)"),
    };
    html.push_str(&facts(&[
        ("Partition key", detail.partition.to_owned()),
        ("Custom event key", custom_key),
        (
            "Reply",
            format!(
                "automatic final answer {}, acknowledgement after {} ms",
                if reply.auto_final { "on" } else { "off" },
                reply.ack_after_ms
            ),
        ),
        ("Reply sinks", sinks),
        (
            "Sources",
            if detail.connection_managed {
                "managed by the channel's connection".to_owned()
            } else {
                format!("{} configured", detail.sources.len())
            },
        ),
    ]));
    html.push_str("<h2>Secrets</h2><p class=\"empty\">Only whether each secret is set is shown, never a value.</p>");
    html.push_str(&facts(&[
        (
            "Agent key",
            format!(
                "{} (hash <code>{}</code>)",
                set(secrets.agent_key),
                escape(&secrets.agent_key_hash)
            ),
        ),
        ("Channel key", set(secrets.channel_key)),
        ("Telegram token", set(secrets.telegram_bot_token)),
        (
            "Telegram webhook secret",
            set(secrets.telegram_webhook_secret),
        ),
        ("HMAC secrets", hmac),
    ]));
    html
}

fn owner_kind(kind: NyxIdIdentityKind) -> &'static str {
    match kind {
        NyxIdIdentityKind::Human => "human",
        NyxIdIdentityKind::ServiceAccount => "service account",
        NyxIdIdentityKind::AgentKey => "agent key",
        NyxIdIdentityKind::Organization => "organization",
    }
}

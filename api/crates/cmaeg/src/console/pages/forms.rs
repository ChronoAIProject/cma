//! The attach forms at the end of the Sources tab, one per source type,
//! mirroring `SourceRequest`. Each posts with its CSRF token and the
//! version the owner saw, so a refreshed post is refused with 409 rather
//! than applied twice. A re-rendered form keeps what the owner typed except
//! secrets: every secret input is always empty.
use cmaeg_core::ids::RecordId;

use super::{channel::path, escape};
use crate::console::attach_form::Submitted;

/// The route template the attach forms post to.
pub(crate) const ATTACH_PATH: &str = "/console/channels/{record_id}/sources";

enum Input {
    Text,
    Secret,
    Area,
    Choice(&'static [(&'static str, &'static str)]),
    Check,
}

struct Field {
    name: &'static str,
    label: &'static str,
    input: Input,
}

const fn field(name: &'static str, label: &'static str, input: Input) -> Field {
    Field { name, label, input }
}

struct Form {
    kind: &'static str,
    title: &'static str,
    fields: &'static [Field],
    admission: bool,
}

const IDENTITY: &[(&str, &str)] = &[
    ("open", "open"),
    ("deny", "denied"),
    ("allowlist", "allowlist"),
];

const ADMISSION: &[Field] = &[
    field(
        "admission",
        "Admission",
        Input::Choice(&[
            ("all", "all senders"),
            ("allowlist", "sender allowlist"),
            ("scoped", "scoped"),
        ]),
    ),
    field(
        "sender_ids",
        "Sender ids (comma or line separated)",
        Input::Area,
    ),
    field("senders", "Scoped: senders", Input::Choice(IDENTITY)),
    field("chats", "Scoped: chats", Input::Choice(IDENTITY)),
    field("chat_ids", "Scoped: chat ids", Input::Area),
    field(
        "groups",
        "Scoped: groups",
        Input::Choice(&[
            ("mention", "on mention"),
            ("reply_to_bot", "on reply to the bot"),
            ("mention_or_reply_to_bot", "on mention or reply"),
            ("all", "all messages"),
            ("deny", "denied"),
        ]),
    ),
];

const FORMS: [Form; 3] = [
    Form {
        kind: "telegram",
        title: "Attach a Telegram bot",
        fields: &[
            field("bot_id", "Bot id", Input::Text),
            field("bot_username", "Bot username", Input::Text),
            field("telegram_bot_token", "Bot token", Input::Secret),
            field("telegram_webhook_secret", "Webhook secret", Input::Secret),
        ],
        admission: true,
    },
    Form {
        kind: "nyxid_relay",
        title: "Attach a NyxID relay",
        fields: &[
            field(
                "platform",
                "Platform",
                Input::Choice(&[
                    ("lark", "Lark"),
                    ("feishu", "Feishu"),
                    ("telegram", "Telegram"),
                    ("discord", "Discord"),
                    ("slack", "Slack"),
                    ("whatsapp", "WhatsApp"),
                ]),
            ),
            field("issuer", "Issuer", Input::Text),
            field("key_id", "Relay key id", Input::Text),
            field(
                "route_ids",
                "Route ids (comma or line separated)",
                Input::Area,
            ),
            field("bot_username", "Bot username", Input::Text),
            field("bot_id", "Bot id", Input::Text),
            field("trust_normalized", "Trust normalized events", Input::Check),
            field(
                "channel_key",
                "Channel key (shared by relays)",
                Input::Secret,
            ),
        ],
        admission: true,
    },
    Form {
        kind: "custom",
        title: "Attach a custom source",
        fields: &[
            field(
                "auth",
                "Authentication",
                Input::Choice(&[("hmac", "HMAC signature"), ("nyxid_key", "NyxID key")]),
            ),
            field("key_id", "HMAC key id", Input::Text),
            field("hmac_secret", "HMAC secret", Input::Secret),
            field(
                "generate_secret",
                "Generate the HMAC secret (shown once)",
                Input::Check,
            ),
            field("key_hashes", "NyxID key hashes", Input::Area),
            field("prompt_template", "Prompt template", Input::Area),
            field("schema", "Event schema (JSON)", Input::Area),
            field("attachments_pointer", "Attachments pointer", Input::Text),
            field("attachments_origins", "Attachment origins", Input::Area),
            field(
                "partition_custom_key",
                "Custom partition key (chat-keyed channels)",
                Input::Choice(&[
                    ("", "none"),
                    ("fixed", "fixed"),
                    ("event_subject", "event_subject"),
                    ("per_event", "per_event"),
                ]),
            ),
        ],
        admission: false,
    },
];

/// The three attach forms for `record` at `version`. `submitted` and
/// `error` re-render the form the owner posted.
pub(crate) fn render(
    record: &RecordId,
    csrf: &str,
    version: u64,
    submitted: Option<&Submitted>,
    error: Option<&str>,
) -> String {
    let action = format!("{}/sources", path(record));
    let mut html = String::from("<h2>Attach a source</h2>");
    for form in &FORMS {
        let mine = submitted.filter(|submitted| submitted.kind() == form.kind);
        let notice = match (mine, error) {
            (Some(_), Some(error)) => format!(
                "<p class=\"notice error\" role=\"alert\">{}</p>",
                escape(error)
            ),
            _ => String::new(),
        };
        let mut fields: String = form
            .fields
            .iter()
            .map(|item| input(form, item, mine))
            .collect();
        if form.admission {
            fields.extend(ADMISSION.iter().map(|item| input(form, item, mine)));
        }
        html.push_str(&format!(
            "<section class=\"panel attach\"><h3>{}</h3>{notice}<form method=\"post\" action=\"{}\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"expected_version\" value=\"{version}\"><input type=\"hidden\" name=\"type\" value=\"{}\">{fields}<button type=\"submit\">Attach</button></form></section>",
            escape(form.title),
            escape(&action),
            escape(csrf),
            form.kind,
        ));
    }
    html
}

fn input(form: &Form, item: &Field, submitted: Option<&Submitted>) -> String {
    let id = format!("{}-{}", form.kind, item.name);
    let value = submitted.map_or("", |submitted| submitted.value(item.name));
    let control = match item.input {
        Input::Text => format!(
            "<input id=\"{id}\" name=\"{}\" value=\"{}\">",
            item.name,
            escape(value)
        ),
        // Never pre-filled, whatever was submitted.
        Input::Secret => format!(
            "<input id=\"{id}\" name=\"{}\" type=\"password\" autocomplete=\"off\" value=\"\">",
            item.name
        ),
        Input::Area => format!(
            "<textarea id=\"{id}\" name=\"{}\" rows=\"2\">{}</textarea>",
            item.name,
            escape(value)
        ),
        Input::Choice(options) => {
            let options: String = options
                .iter()
                .map(|(option, label)| {
                    let selected = if *option == value { " selected" } else { "" };
                    format!("<option value=\"{option}\"{selected}>{label}</option>")
                })
                .collect();
            format!(
                "<select id=\"{id}\" name=\"{}\">{options}</select>",
                item.name
            )
        }
        Input::Check => {
            let checked = if value.is_empty() { "" } else { " checked" };
            format!(
                "<input id=\"{id}\" name=\"{}\" type=\"checkbox\"{checked}>",
                item.name
            )
        }
    };
    format!(
        "<div class=\"field\"><label for=\"{id}\">{}</label>{control}</div>",
        item.label
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::test_support::record;

    #[test]
    fn a_re_rendered_form_keeps_typed_values_but_never_a_secret() {
        let submitted = Submitted::of(
            b"type=telegram&bot_id=%22%3E%3Cimg+src%3Dx%3E&telegram_bot_token=token-value&admission=scoped&groups=all",
        );
        let html = render(
            &record(1),
            "csrf-value",
            7,
            Some(&submitted),
            Some("Refused."),
        );
        assert!(html.contains("value=\"&quot;&gt;&lt;img src=x&gt;\""));
        assert!(!html.contains("<img"));
        assert!(!html.contains("token-value"));
        assert!(html.contains("<option value=\"scoped\" selected>"));
        assert_eq!(html.matches("Refused.").count(), 1);
        assert_eq!(
            html.matches("name=\"expected_version\" value=\"7\"")
                .count(),
            3
        );
        assert_eq!(
            html.matches("type=\"password\" autocomplete=\"off\" value=\"\"")
                .count(),
            4
        );
    }
}

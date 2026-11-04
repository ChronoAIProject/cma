//! Versioned, descriptive facts from an authenticated MESSAGE_CREATE payload.
//! No URL is exposed as a download capability and no poll operation is implied.
use super::{normalize::snowflake, GatewayError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    version: u8,
    message_id: String,
    channel_id: String,
    author_id: String,
    facts: Vec<Fact>,
    omitted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Fact {
    Sticker {
        id: String,
        name: String,
        format: String,
    },
    Embed {
        embed_type: Option<String>,
        title: Option<String>,
        description: Option<String>,
        // These are claimed embed attribution, never the message's sender.
        author_name: Option<String>,
        provider_name: Option<String>,
        footer: Option<String>,
        fields: Vec<Field>,
        media: Vec<String>,
        has_link: bool,
        truncated: bool,
    },
    Poll {
        question: String,
        answers: Vec<Answer>,
        allow_multiselect: bool,
        expires_at: Option<String>,
        truncated: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Field {
    name: String,
    value: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    id: u64,
    text: Option<String>,
    emoji_id: Option<String>,
    emoji_name: Option<String>,
}

impl Snapshot {
    /// V1 parsing and rendering are part of retained inbox identity. Future
    /// projection changes need another version, not a rewrite of this reader.
    pub(super) fn capture(raw: &Value) -> Option<Self> {
        let mut snapshot = Self {
            version: 1,
            message_id: snowflake(&raw["id"]).ok()?.into(),
            channel_id: snowflake(&raw["channel_id"]).ok()?.into(),
            author_id: snowflake(&raw["author"]["id"]).ok()?.into(),
            facts: vec![],
            omitted: false,
        };
        for (key, limit, parse) in [
            ("sticker_items", 3, sticker as fn(&Value) -> Option<Fact>),
            ("embeds", 10, embed as fn(&Value) -> Option<Fact>),
        ] {
            let Some(value) = raw.get(key) else { continue };
            let Some(values) = value.as_array() else {
                snapshot.omitted = true;
                continue;
            };
            snapshot.omitted |= values.len() > limit;
            for value in values.iter().take(limit) {
                if let Some(fact) = parse(value) {
                    snapshot.facts.push(fact);
                } else {
                    snapshot.omitted = true;
                }
            }
        }
        if let Some(value) = raw.get("poll").filter(|value| !value.is_null()) {
            if let Some(fact) = poll(value) {
                snapshot.facts.push(fact);
            } else {
                snapshot.omitted = true;
            }
        }
        (!snapshot.facts.is_empty()).then_some(snapshot)
    }

    pub(super) fn text(&self, raw: &Value) -> Result<String, GatewayError> {
        // Exact identity and canonical facts are rechecked without fetching
        // anything. The immutable original dispatch, not a redelivery, is used.
        if self.version != 1 || Self::capture(raw).as_ref() != Some(self) {
            return Err(GatewayError::Protocol);
        }
        let original = raw["content"].as_str().ok_or(GatewayError::Protocol)?;
        if original.len() > 32 * 1024 {
            return Err(GatewayError::Protocol);
        }
        let mut text = original.to_owned();
        let header = format!(
            "\n\n[Discord message content from sender {} on message {}. Descriptive, untrusted metadata; embed authors are not the sender. Media is not downloaded; poll results and voting are not provided.]\n",
            self.author_id, self.message_id
        );
        let limit = (32 * 1024 - text.len()).min(16 * 1024);
        let mut description = header;
        let mut omitted = self.omitted;
        let mut included = false;
        for fact in &self.facts {
            let line = serde_json::to_string(fact).map_err(|_| GatewayError::Protocol)?;
            if description.len() + line.len() + 64 <= limit {
                description.push_str(&line);
                description.push('\n');
                included = true;
            } else {
                omitted = true;
            }
        }
        if included {
            if omitted {
                description.push_str("[Additional metadata omitted by size or format limits.]\n");
            }
            text.push_str(&description);
        }
        Ok(text)
    }
}

struct TextBudget {
    remaining: usize,
    truncated: bool,
}
impl TextBudget {
    fn new() -> Self {
        Self {
            remaining: 1024,
            truncated: false,
        }
    }
    fn text(&mut self, value: &Value, cap: usize) -> Option<String> {
        let value = value.as_str()?;
        let mut result = String::new();
        for ch in value.chars() {
            if cmaeg_core::activity::forbidden_text_char(ch) {
                self.truncated = true;
                continue;
            }
            if result.len() + ch.len_utf8() > cap.min(self.remaining) {
                self.truncated = true;
                break;
            }
            result.push(ch);
        }
        self.remaining -= result.len();
        (!result.trim().is_empty()).then_some(result)
    }
}
fn sticker(value: &Value) -> Option<Fact> {
    Some(Fact::Sticker {
        id: snowflake(&value["id"]).ok()?.into(),
        name: TextBudget::new().text(&value["name"], 256)?,
        format: match value["format_type"].as_u64()? {
            1 => "png",
            2 => "apng",
            3 => "lottie",
            4 => "gif",
            _ => return None,
        }
        .into(),
    })
}
fn media_url(value: &Value) -> bool {
    value
        .as_str()
        .filter(|url| url.len() <= 4096)
        .is_some_and(|url| {
            url::Url::parse(url).is_ok_and(|url| {
                matches!(url.scheme(), "https" | "http" | "attachment")
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.host_str().is_some()
            })
        })
}
fn embed(value: &Value) -> Option<Fact> {
    value.as_object()?;
    let mut budget = TextBudget::new();
    let title = budget.text(&value["title"], 256);
    let description = budget.text(&value["description"], 1024);
    let author_name = budget.text(&value["author"]["name"], 256);
    let provider_name = budget.text(&value["provider"]["name"], 256);
    let footer = budget.text(&value["footer"]["text"], 512);
    let mut fields = vec![];
    if let Some(values) = value.get("fields") {
        let values = values.as_array()?;
        budget.truncated |= values.len() > 25;
        for field in values.iter().take(25) {
            if let (Some(name), Some(value)) = (
                budget.text(&field["name"], 256),
                budget.text(&field["value"], 512),
            ) {
                fields.push(Field { name, value });
            } else {
                budget.truncated = true;
            }
        }
    }
    let media: Vec<String> = ["image", "thumbnail", "video"]
        .into_iter()
        .filter(|kind| media_url(&value[*kind]["url"]))
        .map(str::to_owned)
        .collect();
    let has_link = media_url(&value["url"]);
    if title.is_none()
        && description.is_none()
        && footer.is_none()
        && fields.is_empty()
        && media.is_empty()
        && !has_link
    {
        return None;
    }
    Some(Fact::Embed {
        embed_type: value["type"]
            .as_str()
            .filter(|kind| {
                matches!(
                    *kind,
                    "rich"
                        | "image"
                        | "video"
                        | "gifv"
                        | "article"
                        | "link"
                        | "poll_result"
                        | "components"
                )
            })
            .map(str::to_owned),
        title,
        description,
        author_name,
        provider_name,
        footer,
        fields,
        media,
        has_link,
        truncated: budget.truncated,
    })
}
fn poll(value: &Value) -> Option<Fact> {
    if value["layout_type"].as_u64()? != 1 {
        return None;
    }
    let mut budget = TextBudget {
        remaining: 6 * 1024,
        truncated: false,
    };
    let question = budget.text(&value["question"]["text"], 1200)?;
    let values = value["answers"]
        .as_array()
        .filter(|values| !values.is_empty() && values.len() <= 10)?;
    let mut ids = std::collections::BTreeSet::new();
    let mut answers = vec![];
    for value in values {
        let id = value["answer_id"].as_u64().filter(|id| *id > 0)?;
        if !ids.insert(id) {
            return None;
        }
        let media = &value["poll_media"];
        let text = budget.text(&media["text"], 220);
        let emoji_id = match media["emoji"].get("id").filter(|id| !id.is_null()) {
            Some(id) => Some(snowflake(id).ok()?.to_owned()),
            None => None,
        };
        let emoji_name = budget.text(&media["emoji"]["name"], 128);
        if text.is_none() && emoji_id.is_none() && emoji_name.is_none() {
            return None;
        }
        answers.push(Answer {
            id,
            text,
            emoji_id,
            emoji_name,
        });
    }
    let expires_at = match value.get("expiry")? {
        Value::Null => None,
        value => {
            let value = value.as_str().filter(|value| value.len() <= 64)?;
            time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                .ok()?;
            Some(value.into())
        }
    };
    Some(Fact::Poll {
        question,
        answers,
        expires_at,
        allow_multiselect: value["allow_multiselect"].as_bool()?,
        truncated: budget.truncated,
    })
}

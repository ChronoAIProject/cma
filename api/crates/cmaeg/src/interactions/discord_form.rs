//! Native modal schema and opaque controls; business identifiers stay on the card.
use super::{Card, Error};
use cmaeg_core::message::{Block, FormField, FormFieldKind, OptionStyle, OutboundMessage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    pub block: usize,
    pub schema_sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    pub interaction: String,
    pub guild: Option<String>,
    pub expires_at: i64,
    /// Claim precedes HTTP. Unknown writes must never repeat the callback.
    pub dispatched: bool,
}

pub(crate) fn schema(message: &OutboundMessage) -> Result<(usize, &str, &[FormField]), Error> {
    message.validate().map_err(|_| Error::Denied)?;
    if message.reply_to.is_some()
        || message.whatsapp_template.is_some()
        || !message.native_attachments.is_empty()
    {
        return Err(Error::Denied);
    }
    let mut found = None;
    for (index, block) in message.blocks.iter().enumerate() {
        match block {
            Block::Form { id, fields } if found.is_none() && fields.len() <= 5 => {
                let mut ids = BTreeSet::new();
                for field in fields {
                    if !ids.insert(&field.id) || field.label.chars().count() > 45 {
                        return Err(Error::Denied);
                    }
                    if let FormFieldKind::Choice { options } = &field.kind {
                        let mut ids = BTreeSet::new();
                        if options.is_empty()
                            || options.len() > 20
                            || options.iter().filter(|option| option.selected).count() > 1
                            || options.iter().any(|option| {
                                option.id.is_empty()
                                    || option.id.len() > 512
                                    || !ids.insert(&option.id)
                                    || option.label.is_empty()
                                    || option.label.chars().count() > 100
                                    || option.style != OptionStyle::Default
                            })
                        {
                            return Err(Error::Denied);
                        }
                    }
                }
                found = Some((index, id.as_str(), fields.as_slice()));
            }
            Block::Form { .. } | Block::Buttons { .. } | Block::Choice { .. } => {
                return Err(Error::Denied)
            }
            _ => {}
        }
    }
    found.ok_or(Error::Denied)
}
pub(crate) fn authority(message: &OutboundMessage) -> Result<Authority, Error> {
    let (block, _, _) = schema(message)?;
    let bytes = serde_json::to_vec(&message.blocks[block]).map_err(|_| Error::Denied)?;
    Ok(Authority {
        block,
        schema_sha256: format!("{:x}", Sha256::digest(bytes)),
    })
}
pub(crate) fn locator(card: &Card, prefix: &str, attempt: &str) -> String {
    format!(
        "{prefix}:{}:{}:{attempt}",
        card.id.replace('-', ""),
        card.revision
    )
}
pub(crate) fn decode(custom: &str, prefix: &str) -> Result<(String, u64, String), Error> {
    if custom.len() > 100 {
        return Err(Error::Denied);
    }
    let parts: Vec<_> = custom.split(':').collect();
    if parts.len() != 4 || parts[0] != prefix {
        return Err(Error::Denied);
    }
    let id = uuid::Uuid::parse_str(parts[1]).map_err(|_| Error::Denied)?;
    let revision = parts[2].parse::<u64>().map_err(|_| Error::Denied)?;
    if id.get_version_num() != 4
        || id.simple().to_string() != parts[1]
        || revision == 0
        || revision.to_string() != parts[2]
    {
        return Err(Error::Denied);
    }
    Ok((id.to_string(), revision, parts[3].into()))
}
pub(crate) fn button(card: &Card) -> Result<Value, Error> {
    let (block, _, _) = schema(&card.message)?;
    Ok(
        json!([{"type":1,"components":[{"type":2,"style":1,"label":"Open form",
        "custom_id":locator(card,"cdf1",&block.to_string())}]}]),
    )
}
pub(crate) fn modal(card: &Card, launch: &Launch) -> Result<Value, Error> {
    let (_, _, fields) = schema(&card.message)?;
    let components: Vec<_> = fields.iter().enumerate().map(|(index, field)| {
        let mut child = match &field.kind {
            FormFieldKind::Text {} => json!({"type":4,"style":2,"max_length":1000}),
            FormFieldKind::Number {} => json!({"type":4,"style":1,"max_length":64,"placeholder":"Number (e.g. 12.5)"}),
            FormFieldKind::Date {} => json!({"type":4,"style":1,"max_length":10,"placeholder":"Date (YYYY-MM-DD)"}),
            FormFieldKind::Choice { options } => json!({"type":3,"min_values":if field.required {1} else {0},"max_values":1,
                "options": options.iter().enumerate().map(|(i,o)| json!({"label":o.label,"value":i.to_string(),"default":o.selected})).collect::<Vec<_>>() }),
        };
        child["custom_id"] = format!("f{index}").into();
        child["required"] = field.required.into();
        json!({"type":18,"label":field.label,"component":child})
    }).collect();
    Ok(
        json!({"type":9,"data":{"custom_id":locator(card,"cdm1",&launch.interaction),"title":"Complete form","components":components}}),
    )
}
pub(crate) fn valid(card: &Card) -> bool {
    let form = card
        .discord
        .as_ref()
        .and_then(|identity| identity.form.as_ref());
    let Some(form) = form else {
        return card.discord_form_launch.is_none()
            && card
                .consumed
                .as_ref()
                .is_none_or(|click| click.form_launch.is_none());
    };
    let Ok(expected) = authority(&card.message) else {
        return false;
    };
    form.block == expected.block
        && form.schema_sha256 == expected.schema_sha256
        && card.discord_form_launch.as_ref().is_none_or(|launch| {
            cmaeg_core::channel::valid_discord_id(&launch.interaction)
                && launch
                    .guild
                    .as_ref()
                    .is_none_or(|id| cmaeg_core::channel::valid_discord_id(id))
                && launch.expires_at > 0
                && launch.expires_at <= card.expires_at
        })
        && card.consumed.as_ref().is_none_or(|click| {
            click.selection.is_none()
                && click.action == form.block
                && card.discord_form_launch.as_ref().is_some_and(|launch| {
                    launch.dispatched && click.form_launch.as_ref() == Some(&launch.interaction)
                })
                && click.submitted.as_ref().is_some_and(|submitted| {
                    super::discord_form_values::valid(&card.message, submitted)
                })
        })
}

/// Number/date semantics are preserved with validated text, never a claimed widget.
pub(crate) fn report(message: &OutboundMessage, report: &mut cmaeg_core::message::RenderingReport) {
    let Ok((index, _, fields)) = schema(message) else {
        return;
    };
    let text_values = fields.iter().any(|field| {
        matches!(
            field.kind,
            FormFieldKind::Number {} | FormFieldKind::Date {}
        )
    });
    for block in &mut report.blocks {
        if block.index as usize == index {
            block.outcome = if text_values {
                cmaeg_core::message::BlockRenderingOutcome::Degraded {
                    how: "discord_modal_number_date_validated_text".into(),
                }
            } else {
                cmaeg_core::message::BlockRenderingOutcome::Exact
            };
        }
    }
    report.outcome = if report.blocks.iter().any(|block| {
        !matches!(
            block.outcome,
            cmaeg_core::message::BlockRenderingOutcome::Exact
        )
    }) {
        cmaeg_core::message::RenderingOutcome::Degraded
    } else {
        cmaeg_core::message::RenderingOutcome::Rendered
    };
}

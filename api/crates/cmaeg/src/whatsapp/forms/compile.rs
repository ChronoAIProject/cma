//! Pure static Flow compilation; business IDs never become component names.
use super::{Error, JSON_VERSION, MAX_JSON_BYTES, SCREEN};
use cmaeg_core::message::{
    Block, BlockRendering, BlockRenderingOutcome, FormField, FormFieldKind, OptionStyle,
    OutboundMessage, RenderingOutcome, RenderingReport,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

pub(crate) struct Compiled {
    pub json: Value,
    pub digest: String,
    pub body: String,
    pub rendering: RenderingReport,
}

pub(crate) fn requested(message: &OutboundMessage) -> bool {
    message
        .blocks
        .iter()
        .any(|b| matches!(b, Block::Form { .. }))
}

pub(crate) fn schema(message: &OutboundMessage) -> Result<(usize, &str, &[FormField]), Error> {
    message.validate().map_err(|_| Error::Invalid)?;
    if message.thread_reply
        || message.reply_to.is_some()
        || message.whatsapp_template.is_some()
        || message.native_media_update.is_some()
        || !message.native_attachments.is_empty()
    {
        return Err(Error::Invalid);
    }
    let mut form = None;
    for (index, block) in message.blocks.iter().enumerate() {
        match block {
            Block::Text { .. } => {}
            Block::Form { id, fields } if form.is_none() => {
                let mut names = BTreeSet::new();
                for field in fields {
                    if !names.insert(&field.id) {
                        return Err(Error::Invalid);
                    }
                    if let FormFieldKind::Choice { options } = &field.kind {
                        let mut ids = BTreeSet::new();
                        if options.is_empty()
                            || options.len() > 20
                            || options.iter().filter(|o| o.selected).count() > 1
                            || options.iter().any(|o| {
                                o.id.is_empty()
                                    || o.id.len() > 512
                                    || !ids.insert(&o.id)
                                    || o.label.is_empty()
                                    || o.label.len() > 200
                                    || o.style != OptionStyle::Default
                            })
                        {
                            return Err(Error::Invalid);
                        }
                    }
                }
                form = Some((index, id.as_str(), fields.as_slice()));
            }
            _ => return Err(Error::Invalid),
        }
    }
    form.ok_or(Error::Invalid)
}

pub(crate) fn compile(message: &OutboundMessage) -> Result<Compiled, Error> {
    let (index, _, fields) = schema(message)?;
    let mut body = Vec::new();
    if let Some(text) = &message.text {
        body.push(text.as_str());
    }
    for block in &message.blocks {
        if let Block::Text { text } = block {
            body.push(text.as_str());
        }
    }
    let body = if body.is_empty() {
        "Please complete this form.".into()
    } else {
        body.join("\n\n")
    };
    if body.is_empty() || body.chars().count() > 1024 {
        return Err(Error::Invalid);
    }
    let mut children = Vec::new();
    let mut defaults = Map::new();
    let mut payload = Map::new();
    let mut characters = "FormSubmit".len();
    let mut adapted = false;
    for (i, field) in fields.iter().enumerate() {
        let name = format!("f{i}");
        let label = if field.label.chars().count() <= 20 {
            field.label.clone()
        } else {
            adapted = true;
            characters += field.label.chars().count();
            children.push(json!({"type":"TextBody", "text":field.label}));
            format!("Field {}", i + 1)
        };
        characters += label.chars().count();
        let mut component = match &field.kind {
            FormFieldKind::Text {} => json!({"type":"TextArea", "max-length":1000}),
            FormFieldKind::Number {} => {
                let helper = "Enter a number, for example -2.5 or 1e3";
                characters += helper.chars().count();
                json!({"type":"TextInput", "input-type":"text", "max-chars":64,
                    "pattern":r"^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?$", "helper-text":helper})
            }
            FormFieldKind::Date {} => json!({"type":"CalendarPicker", "mode":"single"}),
            FormFieldKind::Choice { options } => {
                let options: Vec<_> = options
                    .iter()
                    .enumerate()
                    .map(|(j, option)| {
                        if option.selected {
                            defaults.insert(name.clone(), j.to_string().into());
                        }
                        characters += option.label.chars().count();
                        if option.label.chars().count() <= 30 {
                            json!({"id":j.to_string(), "title":option.label})
                        } else {
                            adapted = true;
                            let title = format!("Option {}", j + 1);
                            characters += title.chars().count();
                            json!({"id":j.to_string(), "title":title, "description":option.label})
                        }
                    })
                    .collect();
                json!({"type":"Dropdown", "data-source":options})
            }
        };
        component["name"] = name.clone().into();
        component["label"] = label.into();
        component["required"] = field.required.into();
        children.push(component);
        payload.insert(name.clone(), format!("${{form.{name}}}").into());
    }
    if characters > 4000 {
        return Err(Error::Invalid);
    }
    children.push(json!({"type":"Footer", "label":"Submit",
        "on-click-action":{"name":"complete", "payload":payload}}));
    let mut form = json!({"type":"Form", "name":"form", "children":children});
    if !defaults.is_empty() {
        form["init-values"] = defaults.into();
    }
    let json = json!({"version":JSON_VERSION, "screens":[{
        "id":SCREEN, "title":"Form", "terminal":true, "success":true, "data":{},
        "layout":{"type":"SingleColumnLayout", "children":[form]}}]});
    if serde_json::to_vec(&json).map_err(|_| Error::Invalid)?.len() > MAX_JSON_BYTES {
        return Err(Error::Invalid);
    }
    let rendering = RenderingReport {
        outcome: if adapted {
            RenderingOutcome::Degraded
        } else {
            RenderingOutcome::Rendered
        },
        blocks: message
            .blocks
            .iter()
            .enumerate()
            .map(|(i, _)| BlockRendering {
                index: i as u16,
                outcome: if i == index && adapted {
                    BlockRenderingOutcome::Degraded {
                        how: "Full long labels are retained above inputs or in choice descriptions"
                            .into(),
                    }
                } else {
                    BlockRenderingOutcome::Exact
                },
            })
            .collect(),
    };
    Ok(Compiled {
        digest: super::hash(&json),
        json,
        body,
        rendering,
    })
}

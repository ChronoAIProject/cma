//! Platform controls derived from an interaction card's message.
use super::card::{Action, Card, Error};
use cmaeg_core::message::{Block, OutboundMessage};
use serde_json::{json, Value};

pub fn has_controls(message: &OutboundMessage) -> bool {
    message.blocks.iter().any(|b| {
        matches!(
            b,
            Block::Buttons { .. } | Block::Choice { .. } | Block::Link { .. }
        )
    })
}
pub(super) fn actions(message: &OutboundMessage) -> Vec<Action> {
    let mut result = Vec::new();
    for block in &message.blocks {
        match block {
            Block::Buttons { id, options } => {
                for option in options {
                    result.push(Action {
                        block: id.clone(),
                        id: option.id.clone(),
                        selected: vec![option.id.clone()],
                    });
                }
            }
            Block::Choice {
                id, options, multi, ..
            } => {
                for option in options {
                    let selected = if *multi {
                        options
                            .iter()
                            .filter(|o| {
                                if o.id == option.id {
                                    !o.selected
                                } else {
                                    o.selected
                                }
                            })
                            .map(|o| o.id.clone())
                            .collect()
                    } else {
                        vec![option.id.clone()]
                    };
                    result.push(Action {
                        block: id.clone(),
                        id: option.id.clone(),
                        selected,
                    });
                }
            }
            _ => {}
        }
    }
    result
}
pub(super) fn keyboard(card: &Card) -> Result<Value, Error> {
    let mut rows = Vec::new();
    let mut index = 0;
    for block in &card.message.blocks {
        match block {
            Block::Buttons { options, .. } | Block::Choice { options, .. } => {
                for option in options {
                    rows.push(json!([{"text":format!("{}{}",if option.selected{"✓ "}else{""},option.label),"callback_data":format!("ce1:{}:{index}",card.id.replace('-',""))}]));
                    index += 1;
                }
            }
            Block::Link { url, label } => {
                rows.push(json!([{"text":label.as_deref().unwrap_or("Open link"),"url":url}]))
            }
            _ => {}
        }
    }
    if rows.len() > 100 {
        return Err(Error::Denied);
    }
    Ok(json!({"inline_keyboard":rows}))
}

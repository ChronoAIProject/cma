//! Bounded response assembly; deltas, committed messages and tools retain their identities.
use super::outcomes::TurnOutcome;
use crate::providers::{OutputItem, ResponsesEvent};
use std::collections::{BTreeMap, BTreeSet};
const MAX_TEXT: usize = 256 * 1024;
const MAX_ITEMS: usize = 256;
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Projection {
    text: BTreeMap<String, String>,
    order: Vec<String>,
    committed: BTreeSet<String>,
    tools: BTreeMap<String, String>,
    started: BTreeSet<String>,
    finished: BTreeSet<String>,
}
impl Projection {
    pub(super) fn validate(&self) -> bool {
        self.text.len() <= MAX_ITEMS
            && self.tools.len() <= MAX_ITEMS
            && self.text.values().map(String::len).sum::<usize>() <= MAX_TEXT
            && self.order.len() == self.text.len()
            && self.order.iter().collect::<BTreeSet<_>>().len() == self.text.len()
            && self.order.iter().all(|id| self.text.contains_key(id))
            && self.committed.iter().all(|id| self.text.contains_key(id))
            && self
                .started
                .iter()
                .chain(&self.finished)
                .all(|id| self.tools.contains_key(id))
            && self
                .text
                .keys()
                .chain(self.tools.keys())
                .all(|id| id.len() <= 512)
            && self.tools.values().all(|name| name.len() <= 512)
    }

    pub fn event(&mut self, event: ResponsesEvent) -> Result<Option<TurnOutcome>, &'static str> {
        let outcome = match event {
            ResponsesEvent::OutputTextDelta { item_id, delta } => {
                self.store(&item_id, &delta, true)?;
                Some(TurnOutcome::TextDelta { text: delta })
            }
            ResponsesEvent::OutputTextDone { item_id, text } => self.commit(item_id, text)?,
            ResponsesEvent::OutputItemAdded { item, .. } => match item {
                OutputItem::Message { id, text_so_far } => {
                    self.store(&id, &text_so_far, false)?;
                    None
                }
                OutputItem::McpCall { id, name, .. }
                | OutputItem::FunctionCall { id, name, .. } => self.tool(id, Some(name), None)?,
                _ => None,
            },
            ResponsesEvent::OutputItemDone { item, .. } => match item {
                OutputItem::Message { id, text_so_far } => {
                    let text = if text_so_far.is_empty() {
                        self.text.get(&id).cloned().unwrap_or_default()
                    } else {
                        text_so_far
                    };
                    self.commit(id, text)?
                }
                OutputItem::McpCall { id, name, status } => {
                    self.tool(id, Some(name), Some(status != "failed"))?
                }
                OutputItem::FunctionCall { id, name, .. } => {
                    self.tool(id, Some(name), Some(true))?
                }
                _ => None,
            },
            ResponsesEvent::McpCallInProgress { item_id } => self.tool(item_id, None, None)?,
            ResponsesEvent::McpCallCompleted { item_id } => self.tool(item_id, None, Some(true))?,
            ResponsesEvent::McpCallFailed { item_id } => self.tool(item_id, None, Some(false))?,
            ResponsesEvent::Completed { response, .. } => {
                let mut final_text = String::new();
                if let Some(output) = response.get("output").and_then(|value| value.as_array()) {
                    for item in output {
                        if item.get("type").and_then(|value| value.as_str()) != Some("message") {
                            continue;
                        }
                        let mut message = String::new();
                        if let Some(parts) = item.get("content").and_then(|value| value.as_array())
                        {
                            for part in parts {
                                if part.get("type").and_then(|value| value.as_str())
                                    == Some("output_text")
                                {
                                    append(
                                        &mut message,
                                        part.get("text")
                                            .and_then(|value| value.as_str())
                                            .unwrap_or_default(),
                                    )?;
                                }
                            }
                        } else if let Some(text) = item.get("text").and_then(|value| value.as_str())
                        {
                            append(&mut message, text)?;
                        }
                        if !message.is_empty() {
                            if !final_text.is_empty() {
                                append(&mut final_text, "\n")?;
                            }
                            append(&mut final_text, &message)?;
                        }
                    }
                }
                if final_text.is_empty() {
                    for id in &self.order {
                        let text = &self.text[id];
                        if !text.is_empty() {
                            if !final_text.is_empty() {
                                append(&mut final_text, "\n")?;
                            }
                            append(&mut final_text, text)?;
                        }
                    }
                }
                Some(TurnOutcome::Completed {
                    final_text: (!final_text.is_empty()).then_some(final_text),
                })
            }
            ResponsesEvent::Failed { error } => Some(TurnOutcome::Failed {
                code: error.code,
                message: error.message,
            }),
            ResponsesEvent::Incomplete { reason } => Some(TurnOutcome::Incomplete { reason }),
            _ => None,
        };
        Ok(outcome)
    }
    fn store(&mut self, id: &str, text: &str, delta: bool) -> Result<(), &'static str> {
        if id.len() > 512 || (!self.text.contains_key(id) && self.text.len() >= MAX_ITEMS) {
            return Err("output_too_large");
        }
        let size: usize = self.text.values().map(String::len).sum();
        let removed = if delta {
            0
        } else {
            self.text.get(id).map_or(0, String::len)
        };
        if size - removed + text.len() > MAX_TEXT {
            return Err("output_too_large");
        }
        if !self.text.contains_key(id) {
            self.order.push(id.to_owned());
        }
        let value = self.text.entry(id.to_owned()).or_default();
        if !delta {
            value.clear();
        }
        value.push_str(text);
        Ok(())
    }
    fn commit(&mut self, id: String, text: String) -> Result<Option<TurnOutcome>, &'static str> {
        self.store(&id, &text, false)?;
        Ok(self
            .committed
            .insert(id)
            .then_some(TurnOutcome::MessageCommitted { text }))
    }
    fn tool(
        &mut self,
        id: String,
        name: Option<String>,
        ok: Option<bool>,
    ) -> Result<Option<TurnOutcome>, &'static str> {
        if id.len() > 512
            || name.as_ref().is_some_and(|name| name.len() > 512)
            || (!self.tools.contains_key(&id) && self.tools.len() >= MAX_ITEMS)
        {
            return Err("output_too_large");
        }
        let retained = self
            .tools
            .entry(id.clone())
            .or_insert_with(|| "unknown".into());
        if let Some(name) = name.filter(|name| !name.is_empty()) {
            *retained = name;
        }
        let name = retained.clone();
        Ok(match ok {
            Some(ok) => self
                .finished
                .insert(id)
                .then_some(TurnOutcome::ToolFinished { name, ok }),
            None => self
                .started
                .insert(id)
                .then_some(TurnOutcome::ToolStarted { name }),
        })
    }
}
fn append(target: &mut String, text: &str) -> Result<(), &'static str> {
    if target.len() + text.len() > MAX_TEXT {
        return Err("output_too_large");
    }
    target.push_str(text);
    Ok(())
}

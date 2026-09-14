use super::{OutputStatus, ResponseEvent, ResponseEventPayload as P, ResponseStatus};
use crate::{Error, Result};
use std::collections::BTreeMap;

#[derive(Default)]
struct Item {
    id: String,
    text: Option<String>,
    stage: Option<u8>,
    done: bool,
}
pub(super) struct State {
    response: String,
    conversation: Option<String>,
    replay: bool,
    started: bool,
    progress: bool,
    pub ended: bool,
    pub connection_error: bool,
    items: BTreeMap<usize, Item>,
}
impl State {
    pub fn new(response: String, conversation: Option<String>, replay: bool) -> Self {
        Self {
            response,
            conversation,
            replay,
            started: false,
            progress: false,
            ended: false,
            connection_error: false,
            items: BTreeMap::new(),
        }
    }
    pub fn accept(&mut self, event: &ResponseEvent) -> Result<()> {
        let invalid =
            || Error::protocol("Responses lifecycle, output identity or content is inconsistent");
        if self.ended {
            return Err(invalid());
        }
        let payload = &event.payload;
        if matches!(payload, P::Error { .. }) {
            self.ended = true;
            self.connection_error = true;
            return Ok(());
        }
        if !self.started && !self.replay && !matches!(payload, P::Created { .. }) {
            return Err(invalid());
        }
        self.started = true;
        if let Some(response) = payload.response() {
            let status = match payload {
                P::Created { .. } => ResponseStatus::Queued,
                P::InProgress { .. } => ResponseStatus::InProgress,
                P::Completed { .. } => ResponseStatus::Completed,
                P::Failed { .. } => ResponseStatus::Failed,
                _ => ResponseStatus::Incomplete,
            };
            if response.id != self.response
                || response.conversation.id.is_empty()
                || self
                    .conversation
                    .as_ref()
                    .is_some_and(|id| id != &response.conversation.id)
                || response.status != status
            {
                return Err(invalid());
            }
            self.conversation
                .get_or_insert_with(|| response.conversation.id.clone());
            match payload {
                P::Created { .. } => {
                    if event.sequence_number != 0 || !response.output.is_empty() {
                        return Err(invalid());
                    }
                }
                P::InProgress { .. } => {
                    if self.progress || !response.output.is_empty() {
                        return Err(invalid());
                    }
                    self.progress = true;
                }
                _ => {
                    self.ended = true;
                    for (index, item) in &self.items {
                        let output = response.output.get(*index).ok_or_else(invalid)?;
                        if !item.done
                            || output.id != item.id
                            || output.status != OutputStatus::Completed
                            || output.content.len() != 1
                            || item.text.as_ref().is_some_and(|text| {
                                output.content.first().map(|part| &part.text) != Some(text)
                            })
                        {
                            return Err(invalid());
                        }
                    }
                    if !self.replay && response.output.len() != self.items.len() {
                        return Err(invalid());
                    }
                }
            }
            return Ok(());
        }
        if !self.progress && !self.replay {
            return Err(invalid());
        }
        let (index, id, content) = match payload {
            P::OutputItemAdded { output_index, item }
            | P::OutputItemDone { output_index, item } => (*output_index, item.id.as_str(), None),
            P::ContentPartAdded {
                output_index,
                item_id,
                content_index,
                ..
            }
            | P::ContentPartDone {
                output_index,
                item_id,
                content_index,
                ..
            }
            | P::OutputTextDelta {
                output_index,
                item_id,
                content_index,
                ..
            }
            | P::OutputTextDone {
                output_index,
                item_id,
                content_index,
                ..
            } => (*output_index, item_id.as_str(), Some(*content_index)),
            _ => return Err(invalid()),
        };
        if id.is_empty()
            || content.is_some_and(|index| index != 0)
            || self
                .items
                .iter()
                .any(|(other, item)| *other != index && item.id == id)
        {
            return Err(invalid());
        }
        if !self.items.contains_key(&index) {
            if !self.replay
                && (!matches!(payload, P::OutputItemAdded { .. }) || index != self.items.len())
            {
                return Err(invalid());
            }
            self.items.insert(
                index,
                Item {
                    id: id.into(),
                    ..Default::default()
                },
            );
        }
        let item = self.items.get_mut(&index).ok_or_else(invalid)?;
        if item.id != id || item.done {
            return Err(invalid());
        }
        let require = |actual: Option<u8>, expected: u8| {
            if actual == Some(expected) || (self.replay && actual.is_none()) {
                Ok(())
            } else {
                Err(invalid())
            }
        };
        match payload {
            P::OutputItemAdded { item: output, .. } => {
                if item.stage.is_some()
                    || output.status != OutputStatus::InProgress
                    || !output.content.is_empty()
                {
                    return Err(invalid());
                }
                item.stage = Some(0);
            }
            P::ContentPartAdded { part, .. } => {
                require(item.stage, 0)?;
                if !part.text.is_empty() {
                    return Err(invalid());
                }
                item.text = Some(String::new());
                item.stage = Some(1);
            }
            P::OutputTextDelta { delta, .. } => {
                require(item.stage, 1)?;
                if let Some(text) = &mut item.text {
                    text.push_str(delta);
                }
                item.stage = Some(1);
            }
            P::OutputTextDone { text, .. } => {
                require(item.stage, 1)?;
                if item.text.as_ref().is_some_and(|old| old != text) {
                    return Err(invalid());
                }
                item.text = Some(text.clone());
                item.stage = Some(2);
            }
            P::ContentPartDone { part, .. } => {
                require(item.stage, 2)?;
                if item.text.as_ref().is_some_and(|old| old != &part.text) {
                    return Err(invalid());
                }
                item.text = Some(part.text.clone());
                item.stage = Some(3);
            }
            P::OutputItemDone { item: output, .. } => {
                require(item.stage, 3)?;
                if output.status != OutputStatus::Completed
                    || output.content.len() != 1
                    || item.text.as_ref().is_some_and(|old| {
                        output.content.first().map(|part| &part.text) != Some(old)
                    })
                {
                    return Err(invalid());
                }
                item.text = Some(output.content[0].text.clone());
                item.done = true;
            }
            _ => return Err(invalid()),
        }
        Ok(())
    }
}

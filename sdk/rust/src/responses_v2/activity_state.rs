use super::{ActivityItem, ActivityStatus as Status, OutputItem, ResponseEventPayload as P};
use crate::{Error, Result};

#[derive(Default)]
pub(super) struct ActivityState {
    snapshot: Option<ActivityItem>,
    stage: Option<&'static str>,
    status: Option<Status>,
    done: bool,
}
impl ActivityState {
    pub fn accept(&mut self, event: &P, replay: bool) -> Result<()> {
        let invalid = || Error::protocol("Responses activity lifecycle is inconsistent");
        let require = |stage| {
            if self.stage == Some(stage) || (replay && self.stage.is_none()) {
                Ok(())
            } else {
                Err(invalid())
            }
        };
        if self.done {
            return Err(invalid());
        }
        match event {
            P::OutputItemAdded {
                item: OutputItem::Activity(output),
                ..
            }
            | P::OutputItemDone {
                item: OutputItem::Activity(output),
                ..
            } => {
                if !empty_body(output)
                    || self
                        .snapshot
                        .as_ref()
                        .is_some_and(|old| !same_call(old, output))
                {
                    return Err(invalid());
                }
                if matches!(event, P::OutputItemAdded { .. }) {
                    if self.stage.is_some() || output.status != Status::InProgress {
                        return Err(invalid());
                    }
                    self.stage = Some("added");
                } else {
                    match output.status {
                        Status::InProgress => return Err(invalid()),
                        Status::Completed => require("completed")?,
                        Status::Failed => require("failed")?,
                    }
                    self.done = true;
                }
                self.snapshot = Some(output.clone());
                self.status = Some(output.status);
            }
            P::McpCallInProgress { .. } => {
                require("added")?;
                self.stage = Some("in_progress");
                self.status = Some(Status::InProgress);
            }
            P::McpCallCompleted { .. } | P::McpCallFailed { .. } => {
                require("in_progress")?;
                let completed = matches!(event, P::McpCallCompleted { .. });
                self.stage = Some(if completed { "completed" } else { "failed" });
                self.status = Some(if completed {
                    Status::Completed
                } else {
                    Status::Failed
                });
            }
            _ => return Err(invalid()),
        }
        Ok(())
    }
    pub fn matches(&self, output: &ActivityItem) -> bool {
        empty_body(output)
            && (self.done || self.stage == Some("in_progress"))
            && self.status == Some(output.status)
            && self
                .snapshot
                .as_ref()
                .is_none_or(|old| same_call(old, output))
    }
}
fn empty_body(item: &ActivityItem) -> bool {
    item.server_label == "cma"
        && item.arguments.is_empty()
        && item.output.is_none()
        && item.error.is_none()
}
fn same_call(a: &ActivityItem, b: &ActivityItem) -> bool {
    a.id == b.id && a.name == b.name && a.server_label == b.server_label
}

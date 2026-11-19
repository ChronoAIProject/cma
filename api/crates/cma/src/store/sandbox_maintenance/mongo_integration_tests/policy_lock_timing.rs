//! Observe the successful settings write through commit for this fixture only.
use mongodb::{bson::Document, event::command::CommandEvent};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Timing {
    pending_fence: Option<(i32, Document, i64)>,
    fenced: Option<(Document, i64, Instant)>,
    pending_commit: Option<i32>,
    pub(super) elapsed: Option<Duration>,
}

impl Timing {
    pub(super) fn observe(&mut self, event: CommandEvent) {
        match event {
            CommandEvent::Started(event)
                if matches!(event.command.get_bool("autocommit"), Ok(false)) =>
            {
                let (Ok(session), Ok(transaction)) = (
                    event.command.get_document("lsid"),
                    event.command.get_i64("txnNumber"),
                ) else {
                    return;
                };
                if event.command_name == "update"
                    && matches!(event.command.get_str("update"), Ok("platform_settings"))
                {
                    self.pending_fence = Some((event.request_id, session.clone(), transaction));
                } else if event.command_name == "commitTransaction"
                    && self.fenced.as_ref().is_some_and(
                        |(fenced_session, fenced_transaction, _)| {
                            fenced_session == session && *fenced_transaction == transaction
                        },
                    )
                {
                    self.pending_commit = Some(event.request_id);
                }
            }
            CommandEvent::Succeeded(event) => {
                if self
                    .pending_fence
                    .as_ref()
                    .is_some_and(|(request, _, _)| *request == event.request_id)
                {
                    let (_, session, transaction) = self.pending_fence.take().unwrap();
                    self.fenced = Some((session, transaction, Instant::now()));
                } else if self.pending_commit == Some(event.request_id) {
                    self.elapsed = self
                        .fenced
                        .as_ref()
                        .map(|(_, _, acquired)| acquired.elapsed());
                }
            }
            _ => {}
        }
    }
}

use super::*;
use mongodb::error::{
    ErrorKind, WriteFailure, TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT,
};

pub(super) fn update_failure(event: &CommandEvent) -> Option<(i32, bool, bool)> {
    match event {
        CommandEvent::Failed(event) if event.command_name == "update" => {
            let code = match event.failure.kind.as_ref() {
                ErrorKind::Command(e) => e.code,
                ErrorKind::Write(WriteFailure::WriteError(e)) => e.code,
                _ => return None,
            };
            Some((
                code,
                event
                    .failure
                    .contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT),
                event.failure.contains_label(TRANSIENT_TRANSACTION_ERROR),
            ))
        }
        CommandEvent::Succeeded(event) if event.command_name == "update" => {
            let code = event
                .reply
                .get_array("writeErrors")
                .ok()?
                .first()?
                .as_document()?
                .get_i32("code")
                .ok()?;
            let labels = event.reply.get_array("errorLabels").ok()?;
            Some((
                code,
                labels
                    .iter()
                    .any(|v| v.as_str() == Some(UNKNOWN_TRANSACTION_COMMIT_RESULT)),
                labels
                    .iter()
                    .any(|v| v.as_str() == Some(TRANSIENT_TRANSACTION_ERROR)),
            ))
        }
        _ => None,
    }
}

pub(super) fn report(events: &[CommandEvent]) {
    for event in events {
        if let Some((code, unknown, transient)) = update_failure(event) {
            eprintln!("purge_update_failure code={code} unknown={unknown} transient={transient}");
        }
        match event {
            CommandEvent::Succeeded(event) if event.command_name == "commitTransaction" => {
                eprintln!(
                    "purge_commit_reply ok={} write_concern_error={}",
                    event.reply.get_f64("ok").ok() == Some(1.0),
                    event.reply.contains_key("writeConcernError")
                );
            }
            CommandEvent::Failed(event) if event.command_name == "commitTransaction" => {
                eprintln!(
                    "purge_commit_failure unknown={} transient={}",
                    event
                        .failure
                        .contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT),
                    event.failure.contains_label(TRANSIENT_TRANSACTION_ERROR)
                );
            }
            _ => {}
        }
    }
}

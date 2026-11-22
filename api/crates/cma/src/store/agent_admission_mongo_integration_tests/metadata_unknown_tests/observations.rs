//! Only protocol facts are retained; no filters, rows, identities or error text.
use super::*;
use mongodb::error::{ErrorKind, WriteFailure};

pub(super) fn observe(event: &CommandEvent) -> Option<Document> {
    match event {
        CommandEvent::Started(event)
            if matches!(event.command_name.as_str(), "find" | "commitTransaction") =>
        {
            let mut row = doc! {
                "phase":"started", "command":&event.command_name,
                "request_id":event.request_id,
            };
            if let Ok(concern) = event.command.get_document("readConcern") {
                // Copy the known concern fields only, never an entire command.
                for key in ["level", "atClusterTime", "afterClusterTime"] {
                    if let Some(value) = concern.get(key) {
                        row.insert(key, value.clone());
                    }
                }
            }
            Some(row)
        }
        CommandEvent::Succeeded(event)
            if matches!(event.command_name.as_str(), "find" | "commitTransaction") =>
        {
            let mut row = doc! {
                "phase":"succeeded", "command":&event.command_name,
                "request_id":event.request_id,
            };
            if let Ok(time) = event.reply.get_timestamp("operationTime") {
                row.insert("operationTime", time);
            }
            if let Ok(error) = event.reply.get_document("writeConcernError") {
                if let Ok(code) = error.get_i32("code") {
                    row.insert("writeConcernErrorCode", code);
                }
            }
            if let Ok(cursor) = event.reply.get_document("cursor") {
                if let Ok(time) = cursor.get_timestamp("atClusterTime") {
                    row.insert("atClusterTime", time);
                }
                if let Ok(batch) = cursor.get_array("firstBatch") {
                    row.insert("row_count", batch.len() as i64);
                }
            }
            Some(row)
        }
        CommandEvent::Failed(event)
            if matches!(
                event.command_name.as_str(),
                "find" | "commitTransaction" | "insert"
            ) =>
        {
            let mut row = doc! {
                "phase":"failed", "command":&event.command_name,
                "request_id":event.request_id,
                "unknown":event.failure.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT),
                "transient":event.failure.contains_label(mongodb::error::TRANSIENT_TRANSACTION_ERROR),
            };
            let code = match event.failure.kind.as_ref() {
                ErrorKind::Command(error) => Some(error.code),
                ErrorKind::Write(WriteFailure::WriteError(error)) => Some(error.code),
                ErrorKind::Write(WriteFailure::WriteConcernError(error)) => Some(error.code),
                _ => None,
            };
            if let Some(code) = code {
                row.insert("error_code", code);
            }
            Some(row)
        }
        _ => None,
    }
}

pub(super) fn assert_committed_replies(observations: &[Document]) {
    let commits = observations
        .iter()
        .filter(|row| {
            row.get_str("command").ok() == Some("commitTransaction")
                && row.get_str("phase").ok() == Some("succeeded")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        commits.len(),
        5,
        "the commit command must execute five times"
    );
    assert!(commits
        .iter()
        .all(|row| row.get_i32("writeConcernErrorCode").ok() == Some(50)));
    let at = commits
        .iter()
        .map(|row| row.get_timestamp("operationTime").unwrap())
        .max()
        .unwrap();
    let last_commit = observations
        .iter()
        .rposition(|row| row.get_str("command").ok() == Some("commitTransaction"))
        .unwrap();
    let reads = observations[last_commit + 1..]
        .iter()
        .filter(|row| row.get_str("phase").ok() == Some("started"));
    for read in reads {
        assert_eq!(read.get_str("level").ok(), Some("snapshot"));
        assert_eq!(read.get_timestamp("atClusterTime").ok(), Some(at));
    }
}

//! Keep migration transactions safe across caller cancellation.
use super::{database_error, Acknowledgment, ReadConcern, Result, WriteConcern};
use mongodb::{
    bson::{doc, Bson, Document},
    error::ErrorKind,
    ClientSession, Database,
};
use tokio::task::JoinHandle;
use tracing::instrument::WithSubscriber;

pub(super) async fn start(db: &Database) -> Result<ClientSession> {
    let mut session = db.client().start_session().await.map_err(database_error)?;
    session
        .start_transaction()
        .read_concern(ReadConcern::snapshot())
        .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
        .await
        .map_err(database_error)?;
    // A cancelled first command can still run on the server. Open with a
    // collectionless read, then let session drop abort any started writes.
    db.run_command(doc! {
        "aggregate": 1_i32,
        "pipeline": [{"$documents": [doc! {}]}],
        "cursor": {},
    })
    .session(&mut session)
    .await
    .map_err(database_error)?;
    Ok(session)
}

const UNKNOWN_COMMIT: &str = "migration commit outcome unknown; read status before retry";

pub(super) async fn commit(mut session: ClientSession) -> Result<()> {
    // The driver marks the local transaction terminal before awaiting the
    // command. The terminal task owns the session even if the caller is dropped.
    tokio::spawn(async move {
        for _ in 0..3 {
            match session.commit_transaction().await {
                Ok(()) => return Ok(()),
                Err(error) if error.contains_label("UnknownTransactionCommitResult") => {}
                Err(error) => return Err(database_error(error)),
            }
        }
        Err(UNKNOWN_COMMIT)
    })
    .await
    .map_err(|_| UNKNOWN_COMMIT)?
}

pub(super) async fn abort_after(
    mut session: ClientSession,
    operation: &'static str,
    reason: &'static str,
) -> &'static str {
    let task = tokio::spawn(
        async move {
            // The driver suppresses server abort errors. Observe the response on
            // the original transaction first, then let the driver close its state.
            let reply = session
                .client()
                .database("admin")
                .run_command(doc! {"abortTransaction": 1_i32, "writeConcern": {"w": "majority"}})
                .session(&mut session)
                .await;
            match reply {
                Ok(reply) if acknowledged_abort(&reply) => {}
                Ok(_) => tracing::warn!(operation, "migration abort acknowledgment was uncertain"),
                Err(error) => match error.kind.as_ref() {
                    // MongoDB has already ended this transaction.
                    ErrorKind::Command(error) if error.code == 251 => {}
                    ErrorKind::Command(error) => {
                        tracing::warn!(
                            operation,
                            code = error.code,
                            "migration abort command failed"
                        );
                    }
                    _ => tracing::warn!(operation, "migration abort command failed"),
                },
            }
            if session.abort_transaction().await.is_err() {
                tracing::warn!(
                    operation,
                    "migration driver could not close the transaction"
                );
            }
        }
        .with_current_subscriber(),
    );
    finish_abort(task, operation, reason).await
}

fn acknowledged_abort(reply: &Document) -> bool {
    let ok = matches!(reply.get("ok"), Some(Bson::Int32(1) | Bson::Int64(1)))
        || matches!(reply.get("ok"), Some(Bson::Double(value)) if *value == 1.0);
    ok && !reply.contains_key("writeConcernError") && !reply.contains_key("errorLabels")
}

async fn finish_abort(
    task: JoinHandle<()>,
    operation: &'static str,
    reason: &'static str,
) -> &'static str {
    if let Err(error) = task.await {
        tracing::warn!(
            operation,
            cancelled = error.is_cancelled(),
            "migration abort task did not finish"
        );
    }
    reason
}

#[cfg(test)]
#[path = "transaction_tests.rs"]
mod tests;

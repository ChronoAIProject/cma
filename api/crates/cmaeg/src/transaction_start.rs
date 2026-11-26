use futures_util::{future::BoxFuture, FutureExt};
use mongodb::{
    bson::doc,
    error::Result,
    options::{Acknowledgment, ReadConcern, TransactionOptions, WriteConcern},
    ClientSession, Database,
};
use std::time::Duration;

/// An in-flight first command can outlive the Rust future that sent it.
/// Start with a collectionless read so cancellation at that point leaves no
/// write lock. After the read returns, dropping the driver session can abort
/// a transaction that has started issuing writes.
pub(crate) async fn start_write(
    database: &Database,
    session: &mut ClientSession,
    max_commit_time: Option<Duration>,
) -> Result<()> {
    session
        .start_transaction()
        .with_options(write_options(max_commit_time))
        .await?;
    establish(database, session).await
}

/// Keep the driver's retry and commit semantics while holding the explicit
/// session until a cancelled write transaction is settled.
pub(crate) async fn and_run_write<T, C, F>(database: &Database, context: C, run: F) -> Result<T>
where
    T: Send,
    C: Send,
    F: for<'a> FnMut(&'a mut ClientSession, &'a mut C) -> BoxFuture<'a, Result<T>> + Send,
{
    let mut owned = WriteSession {
        session: Some(database.client().start_session().await?),
        started_writes: false,
    };
    let result = owned
        .session
        .as_mut()
        .expect("write transaction retains its session")
        .start_transaction()
        .with_options(write_options(None))
        .and_run(
            (database, context, run, &mut owned.started_writes),
            |session, (database, context, run, started_writes)| {
                async move {
                    establish(database, session).await?;
                    **started_writes = true;
                    run(session, context).await
                }
                .boxed()
            },
        )
        .await;
    owned.started_writes = false;
    result
}

struct WriteSession {
    session: Option<ClientSession>,
    started_writes: bool,
}

impl Drop for WriteSession {
    fn drop(&mut self) {
        if !self.started_writes {
            return;
        }
        if let Some(session) = self.session.take() {
            crate::transaction_terminal::end_owned_session(session);
        }
    }
}

fn write_options(max_commit_time: Option<Duration>) -> TransactionOptions {
    let mut options = TransactionOptions::builder()
        .read_concern(ReadConcern::snapshot())
        .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
        .build();
    options.max_commit_time = max_commit_time;
    options
}

async fn establish(database: &Database, session: &mut ClientSession) -> Result<()> {
    database
        .run_command(doc! {
            "aggregate": 1_i32,
            "pipeline": [{"$documents": [doc! {}]}],
            "cursor": {},
        })
        .session(session)
        .await?;
    Ok(())
}

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
    let mut options = TransactionOptions::builder()
        .read_concern(ReadConcern::snapshot())
        .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
        .build();
    options.max_commit_time = max_commit_time;
    session.start_transaction().with_options(options).await?;
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

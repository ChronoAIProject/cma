//! Exact DDL admission and acknowledgement fence for service migrations.
use super::*;
use crate::ledger::Lease;

#[derive(Clone)]
struct Execution {
    runner: Runner,
    lease: Lease,
}
tokio::task_local! { static CURRENT: Execution; }

pub(super) async fn scope<T>(
    runner: Runner,
    lease: Lease,
    future: impl std::future::Future<Output = T>,
) -> T {
    CURRENT.scope(Execution { runner, lease }, future).await
}
pub(super) fn active() -> bool {
    CURRENT.try_with(|_| ()).is_ok()
}
pub(super) async fn check_fence() -> mongodb::error::Result<()> {
    if let Ok(context) = CURRENT.try_with(Clone::clone) {
        context
            .runner
            .renew(&context.lease)
            .await
            .map_err(mongodb::error::Error::custom)?;
    }
    Ok(())
}
/// Preserve the original driver's result; migration-only metadata never enters
/// schema records. An unresolved dispatch holds the service ledger barrier.
pub async fn effect<T>(
    db: &Database,
    descriptor: Document,
    future: impl std::future::Future<Output = mongodb::error::Result<T>>,
) -> mongodb::error::Result<T> {
    let Ok(context) = CURRENT.try_with(Clone::clone) else {
        return future.await;
    };
    if db.name() != context.runner.db.name() {
        return Err(mongodb::error::Error::custom("migration database changed"));
    }
    let mut digest = Sha256::new();
    digest.update(
        mongodb::bson::serialize_to_vec(&descriptor).map_err(mongodb::error::Error::custom)?,
    );
    let digest = format!("{:x}", digest.finalize());
    let label = descriptor
        .keys()
        .next()
        .cloned()
        .ok_or_else(|| mongodb::error::Error::custom("empty migration effect"))?;
    context
        .runner
        .begin_effect(&context.lease, &digest, &label, &descriptor)
        .await
        .map_err(mongodb::error::Error::custom)?;
    tokio::pin!(future);
    let mut renewal = tokio::time::interval(Duration::from_secs(5));
    let result = loop {
        tokio::select! {biased; _=renewal.tick()=>context.runner.renew(&context.lease).await.map_err(mongodb::error::Error::custom)?, result=&mut future=>break result}
    };
    // A definite server refusal has no remaining dispatch continuation. The
    // generic migration can re-inspect and resume after such a reply.
    if result.is_ok()
        || result.as_ref().err().is_some_and(|error| {
            matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(_))
                && !error.contains_label("UnknownTransactionCommitResult")
                && !error.contains_label("RetryableWriteError")
        })
    {
        context
            .runner
            .end_effect(
                &context.lease,
                &digest,
                if result.is_ok() {
                    "acknowledged"
                } else {
                    "server_refused"
                },
            )
            .await
            .map_err(mongodb::error::Error::custom)?;
    }
    result
}
pub async fn command(db: &Database, mut command: Document) -> mongodb::error::Result<Document> {
    if CURRENT.try_with(|_| ()).is_ok() {
        command.insert("writeConcern", doc! {"w":"majority"});
    }
    effect(db, command.clone(), async {
        let response = db.run_command(command).await?;
        if response.contains_key("writeConcernError") {
            return Err(mongodb::error::Error::custom(
                "migration command acknowledgement uncertain",
            ));
        }
        Ok(response)
    })
    .await
}

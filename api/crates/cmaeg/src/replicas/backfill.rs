//! One-time compatibility gate after the explicitly coordinated no-overlap stop.
use super::{
    store::{Store, Write},
    Error,
};
use crate::{connections::store::index_id, ingress::routing_store::PlanStore};
use serde::{Deserialize, Serialize};

#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Complete {
    kind: String,
    deployment: String,
}

pub(crate) async fn run(store: &Store, plans: &PlanStore, deployment: &str) -> Result<(), Error> {
    let id = index_id("cmaeg-replica-legacy-backfill-v1", &[deployment]);
    let expected = Complete {
        kind: "cmaeg_replica_legacy_backfill_v1".into(),
        deployment: deployment.into(),
    };
    if let Some((saved, _)) = store.read::<Complete>(&id, false).await? {
        return if saved == expected {
            Ok(())
        } else {
            Err(Error::Corrupt)
        };
    }
    // New members do not become serving replicas until every retained legacy
    // root has scheduling custody. A failed page leaves this gate incomplete.
    let started = std::time::Instant::now();
    let mut cursor = None;
    let mut pages = 0u64;
    loop {
        let page = plans
            .recover_legacy_page(cursor.as_deref(), 32)
            .await
            .map_err(|_| Error::Unavailable)?;
        if !page.issues.is_empty() {
            return Err(Error::Corrupt);
        }
        pages += 1;
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    match store
        .write(&id, deployment, &expected, Write::default())
        .await
    {
        Ok(_) => {}
        Err(Error::Busy) => match store.read::<Complete>(&id, false).await? {
            Some((saved, _)) if saved == expected => {}
            _ => return Err(Error::Corrupt),
        },
        Err(error) => return Err(error),
    }
    tracing::info!(
        pages,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "replica legacy admission backfill completed"
    );
    Ok(())
}

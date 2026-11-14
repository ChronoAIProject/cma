//! Activation is one-way until a separately reviewed all-writers-stopped migration.
use super::{
    store::{Store, Write},
    Error,
};
use crate::connections::store::index_id;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Floor {
    kind: String,
    protocol: u32,
    deployment: String,
}

pub(crate) async fn enforce(store: &Store, origin: &str, enabled: bool) -> Result<(), Error> {
    // A global record binds the deployment namespace too. Different public
    // origins must not accidentally split authority over the same database.
    let id = index_id("cmaeg-replica-protocol-floor-v1", &[]);
    let expected = Floor {
        kind: "cmaeg_replica_protocol_floor_v1".into(),
        protocol: 1,
        deployment: format!("{:x}", Sha256::digest(origin.as_bytes())),
    };
    if let Some((floor, _)) = store.read::<Floor>(&id, false).await? {
        return if enabled && floor == expected {
            Ok(())
        } else {
            Err(Error::Corrupt)
        };
    }
    if !enabled {
        return Ok(());
    }
    match store
        .write(&id, &expected.deployment, &expected, Write::default())
        .await
    {
        Ok(_) => Ok(()),
        Err(Error::Busy) => match store.read::<Floor>(&id, false).await? {
            Some((floor, _)) if floor == expected => Ok(()),
            _ => Err(Error::Corrupt),
        },
        Err(error) => Err(error),
    }
}

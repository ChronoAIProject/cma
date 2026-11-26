use std::sync::RwLock;

use layer2_core::StoreError;
use layer3_adapter::{
    ReleasedLaneRevisions, ReleasedRuntime, ReleasedRuntimeSource, StaleReleasedRuntime,
};
use mongodb::bson::doc;

use super::*;

pub(crate) struct StoreReleasedRuntime {
    pub(crate) lanes: MongoReleasedRuntimes,
    current: RwLock<ReleasedRuntime>,
    refreshed: tokio::sync::Mutex<()>,
    machine: bool,
}

impl std::fmt::Debug for StoreReleasedRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoreReleasedRuntime")
            .finish_non_exhaustive()
    }
}

impl StoreReleasedRuntime {
    pub(crate) async fn open(
        database: &Database,
        deployment: &str,
        pins: ReleasedRuntime,
        required: bool,
    ) -> Result<Option<Self>, StoreError> {
        let source = Self {
            lanes: MongoReleasedRuntimes::open(database, deployment),
            machine: pins.machine.is_some(),
            current: RwLock::new(pins),
            refreshed: tokio::sync::Mutex::new(()),
        };
        if !required {
            // Legacy roles may lack the optional collection. The required pool
            // collection retains a durable floor once a lane starts bootstrapping.
            let read = source
                .lanes
                .lanes
                .find_one(doc! {"_id": source.lanes.id(ReleasedLane::Agent)})
                .await;
            match read {
                Ok(None) => source.require_never_enabled(database, deployment).await?,
                Err(error) if matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(command) if command.code == 13) => {
                    source.require_never_enabled(database, deployment).await?
                }
                Err(_) => {
                    return Err(StoreError::Unavailable(
                        "read durable runtime authority".into(),
                    ))
                }
                Ok(Some(_)) => {
                    source
                        .refresh()
                        .await
                        .map_err(|error| StoreError::Unavailable(error.to_string()))?;
                    return Ok(Some(source));
                }
            }
            return Ok(None);
        }
        source
            .refresh()
            .await
            .map_err(|error| StoreError::Unavailable(error.to_string()))?;
        Ok(Some(source))
    }

    async fn require_never_enabled(
        &self,
        database: &Database,
        deployment: &str,
    ) -> Result<(), StoreError> {
        let targets: Collection<mongodb::bson::Document> =
            majority_collection(database, super::super::native_warm_pools::COLLECTION);
        let footprint = targets
            .find_one(doc! {"plan.deployment": deployment, "durable_release_authority": true})
            .projection(doc! {"_id": 1})
            .await
            .map_err(|_| {
                StoreError::Unavailable("verify runtime release authority floor".into())
            })?;
        if footprint.is_some() {
            return Err(StoreError::Unavailable(
                "deployment has unreadable durable runtime release authority".into(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn refresh(&self) -> Result<ReleasedRuntime, SandboxStoreError> {
        let _refresh = self.refreshed.lock().await;
        let old = self.snapshot();
        let agent = self.lanes.load(ReleasedLane::Agent).await?;
        let machine = self.lanes.load(ReleasedLane::Machine).await?;
        let Some(agent) = agent else {
            if old.lane_revisions.is_some() || machine.is_some() {
                return Err(SandboxStoreError::Corrupt(
                    "durable agent release is missing".into(),
                ));
            }
            return Ok(old);
        };
        if !self.machine && machine.is_some() {
            return Err(SandboxStoreError::Corrupt(
                "recorded machine release requires its lane configuration".into(),
            ));
        }
        if old.lane_revisions.is_some_and(|old| {
            agent.active.revision < old.agent
                || old.machine.is_some_and(|prior| {
                    machine
                        .as_ref()
                        .is_none_or(|record| record.active.revision < prior)
                })
        }) {
            return Err(SandboxStoreError::Corrupt(
                "released runtime revision regressed".into(),
            ));
        }
        let revisions = ReleasedLaneRevisions {
            agent: agent.active.revision,
            machine: machine.as_ref().map(|record| record.active.revision),
        };
        let revision = revisions
            .agent
            .checked_add(revisions.machine.unwrap_or_default())
            .ok_or_else(|| {
                SandboxStoreError::Corrupt("released runtime revision overflow".into())
            })?;
        let release = ReleasedRuntime {
            revision,
            agent: agent.active.image,
            machine: machine.map(|record| record.active.image).or(old.machine),
            lane_revisions: Some(revisions),
        };
        *self.current.write().expect("released runtime lock") = release.clone();
        Ok(release)
    }
}

#[async_trait::async_trait]
impl ReleasedRuntimeSource for StoreReleasedRuntime {
    fn snapshot(&self) -> ReleasedRuntime {
        self.current.read().expect("released runtime lock").clone()
    }

    fn require_current(&self, released: &ReleasedRuntime) -> Result<(), StaleReleasedRuntime> {
        if self.snapshot() == *released {
            Ok(())
        } else {
            Err(StaleReleasedRuntime)
        }
    }

    async fn refresh_after_stale(&self) {
        if let Err(error) = self.refresh().await {
            tracing::warn!(%error, "durable released runtime refresh deferred");
        }
    }
}

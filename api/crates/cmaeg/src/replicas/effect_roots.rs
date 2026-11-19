//! Indexed effect intents retain uncertainty while avoiding full receipt scans.
use super::{
    partition::{PartitionLease, Partitions},
    store::Write,
    Error,
};
use crate::records::mongo::{ReplicaKind, ReplicaProjection};
use serde::Serialize;

impl Partitions {
    fn effect_projection(&self, active: bool) -> ReplicaProjection {
        ReplicaProjection {
            kind: ReplicaKind::Effect,
            deployment: self.membership.deployment.clone(),
            active,
        }
    }
    pub(crate) async fn effect_roots(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, serde_json::Value, u64)>, Error> {
        self.store
            .active(
                ReplicaKind::Effect,
                &self.membership.deployment,
                after,
                limit,
            )
            .await
    }
    pub(crate) async fn write_effect_root(
        &self,
        id: &str,
        owner: &str,
        retained_until: i64,
        value: &impl Serialize,
    ) -> Result<u64, Error> {
        self.store
            .write(
                id,
                owner,
                value,
                Write {
                    retained_until: Some(retained_until),
                    projection: Some(self.effect_projection(true)),
                    ..Default::default()
                },
            )
            .await
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn write_effect_fenced(
        &self,
        token: &PartitionLease,
        id: &str,
        owner: &str,
        retained_until: i64,
        value: &impl Serialize,
        expected: Option<u64>,
        active: bool,
    ) -> Result<u64, Error> {
        self.write_projected(
            token,
            id,
            owner,
            retained_until,
            value,
            expected,
            Some(self.effect_projection(active && !token.is_lane())),
        )
        .await
    }
}

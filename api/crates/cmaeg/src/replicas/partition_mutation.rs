//! Existing journals compose their own CAS with this captured resource fence.
use super::{
    partition::{PartitionLease, Partitions},
    store::Write,
    Error,
};
use serde::Serialize;

impl Partitions {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn write(
        &self,
        token: &PartitionLease,
        id: &str,
        owner: &str,
        retained_until: i64,
        value: &impl Serialize,
        expected: Option<u64>,
    ) -> Result<u64, Error> {
        self.write_projected(token, id, owner, retained_until, value, expected, None)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn write_projected(
        &self,
        token: &PartitionLease,
        id: &str,
        owner: &str,
        retained_until: i64,
        value: &impl Serialize,
        expected: Option<u64>,
        projection: Option<crate::records::mongo::ReplicaProjection>,
    ) -> Result<u64, Error> {
        if token.owner != owner {
            return Err(Error::Lost);
        }
        for _ in 0..4 {
            let guard = self.guard(token).await?;
            let result = self
                .store
                .write(
                    id,
                    owner,
                    value,
                    Write {
                        expected,
                        retained_until: Some(retained_until),
                        guard: Some(&guard),
                        projection: projection.clone(),
                        ..Default::default()
                    },
                )
                .await;
            match result {
                Ok(version) => return Ok(version),
                // Refresh physical version after a concurrent append, under the
                // same captured authority. Target CAS and ambiguous commit errors
                // are returned rather than interpreted as safe effect retries.
                Err(Error::Lost) if token.live() => continue,
                Err(error) => return Err(error),
            }
        }
        Err(Error::Busy)
    }
}

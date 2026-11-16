//! Immutable per-process capability; only the original Member row grants liveness.
use super::*;
use futures_util::TryStreamExt;
use mongodb::bson::doc;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Capability {
    kind: String,
    deployment: String,
    instance: String,
    quarantine: bool,
}
fn id(instance: &str) -> String {
    crate::connections::store::index_id("replica-member-capability-v1", &[instance])
}
pub(super) async fn publish(store: &Store, member: &Member) -> Result<(), Error> {
    let value = Capability {
        kind: "replica_member_capability_v1".into(),
        deployment: member.deployment.clone(),
        instance: member.instance.clone(),
        quarantine: true,
    };
    let retained_until = store.clock().await?.millis.div_euclid(1000) + 30 * 86400;
    store
        .write(
            &id(&member.instance),
            &member.deployment,
            &value,
            Write {
                retained_until: Some(retained_until),
                ..Default::default()
            },
        )
        .await?;
    Ok(())
}
#[derive(Clone, Serialize)]
pub(crate) struct MemberCapability {
    pub instance: String,
    pub origin: String,
    pub quarantine: bool,
    pub draining: bool,
}
impl Membership {
    /// Includes draining members: readiness withdrawal does not cancel live IO.
    pub(crate) async fn capabilities(&self, store: &Store) -> Result<Vec<MemberCapability>, Error> {
        let now = store.clock().await?.millis;
        let rows: Vec<mongodb::bson::Document> =
            tokio::time::timeout(super::super::config::STORE_TIMEOUT, async {
                store
                    .collection(true)?
                    .find(doc! {"replica.kind":"member", "replica.deployment":&self.deployment})
                    .limit(129)
                    .await
                    .map_err(|_| Error::Unavailable)?
                    .try_collect()
                    .await
                    .map_err(|_| Error::Unavailable)
            })
            .await
            .map_err(|_| Error::Unavailable)??;
        if rows.len() > 128 {
            return Err(Error::Capacity);
        }
        let mut output = Vec::new();
        for row in rows {
            let key = row.get_str("_id").map_err(|_| Error::Corrupt)?.to_owned();
            let (member, _) = store.decode::<Member>(&key, true, row)?;
            if member.until_ms <= now {
                continue;
            }
            if member.kind != "cmaeg_replica_member_v1"
                || member.instance.len() != 64
                || member.deployment != self.deployment
                || member.protocol != PROTOCOL
            {
                return Err(Error::Corrupt);
            }
            let capability = store
                .read::<Capability>(&id(&member.instance), true)
                .await?;
            let quarantine = match capability {
                Some((c, 1))
                    if c.kind == "replica_member_capability_v1"
                        && c.instance == member.instance
                        && c.deployment == self.deployment =>
                {
                    c.quarantine
                }
                None => false,
                _ => return Err(Error::Corrupt),
            };
            let origin = super::super::config::validate_address(&member.origin)
                .map_err(|_| Error::Corrupt)?;
            output.push(MemberCapability {
                instance: member.instance,
                origin,
                quarantine,
                draining: member.draining,
            });
        }
        if output.is_empty() {
            return Err(Error::Lost);
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires authenticated disposable Mongo replica set"]
    async fn companion_is_expiring_and_cannot_grant_liveness_after_its_horizon() {
        let fixture = crate::replicas::mongo_fixture::Fixture::new().await;
        let node = fixture.node(1).await;
        let store = &node.partitions.store;
        let key = id(&node.partitions.membership.instance);
        let now = store.clock().await.unwrap().millis;
        let rows = store.collection(true).unwrap();
        let row = rows.find_one(doc! {"_id":&key}).await.unwrap().unwrap();
        let expiry = row.get_datetime("expires_at").unwrap().timestamp_millis();
        assert!((30 * 86400 * 1000 - 5000..=30 * 86400 * 1000).contains(&(expiry - now)));
        assert!(store
            .read::<serde_json::Value>(&key, false)
            .await
            .unwrap()
            .is_none());
        let (value, version) = store
            .read::<serde_json::Value>(&key, true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(version, 1);
        assert!(value.get("build").is_none());
        rows.update_one(
            doc! {"_id":&key},
            doc! {"$set":{"expires_at":mongodb::bson::DateTime::from_millis(now-1000)}},
        )
        .await
        .unwrap();
        let members = node
            .partitions
            .membership
            .capabilities(store)
            .await
            .unwrap();
        assert_eq!(members.len(), 1);
        assert!(!members[0].quarantine);
        drop(node);
        fixture.cleanup().await;
    }
}

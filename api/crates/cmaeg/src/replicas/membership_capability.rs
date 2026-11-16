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
    build: String,
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
        build: env!("CARGO_PKG_VERSION").into(),
        quarantine: true,
    };
    store
        .write(
            &id(&member.instance),
            &member.deployment,
            &value,
            Write::default(),
        )
        .await?;
    Ok(())
}
#[derive(Clone, Serialize)]
pub(crate) struct MemberCapability {
    pub instance: String,
    pub build: Option<String>,
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
            if member.deployment != self.deployment || member.protocol != PROTOCOL {
                return Err(Error::Corrupt);
            }
            let capability = store
                .read::<Capability>(&id(&member.instance), false)
                .await?;
            let (build, quarantine) = match capability {
                Some((c, 1))
                    if c.kind == "replica_member_capability_v1"
                        && c.instance == member.instance
                        && c.deployment == self.deployment =>
                {
                    (Some(c.build), c.quarantine)
                }
                None => (None, false),
                _ => return Err(Error::Corrupt),
            };
            output.push(MemberCapability {
                instance: member.instance,
                build,
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

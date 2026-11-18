//! Separate companion: PR A's strict quarantine-capability shape stays unchanged.
use super::*;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaneCapability {
    kind: String,
    deployment: String,
    instance: String,
    delayed_lane_v1: bool,
}
fn id(instance: &str) -> String {
    crate::connections::store::index_id("replica-delayed-lane-capability-v1", &[instance])
}
pub(super) async fn publish(store: &Store, member: &Member) -> Result<(), Error> {
    let value = LaneCapability {
        kind: "replica_delayed_lane_capability_v1".into(),
        deployment: member.deployment.clone(),
        instance: member.instance.clone(),
        delayed_lane_v1: true,
    };
    store
        .write(
            &id(&member.instance),
            &member.deployment,
            &value,
            Write {
                retained_until: Some(store.clock().await?.millis / 1000 + 30 * 86400),
                ..Default::default()
            },
        )
        .await?;
    Ok(())
}
pub(super) async fn supports(store: &Store, member: &Member) -> Result<bool, Error> {
    match store
        .read::<LaneCapability>(&id(&member.instance), true)
        .await?
    {
        Some((value, 1))
            if value.kind == "replica_delayed_lane_capability_v1"
                && value.deployment == member.deployment
                && value.instance == member.instance =>
        {
            Ok(value.delayed_lane_v1)
        }
        None => Ok(false),
        _ => Err(Error::Corrupt),
    }
}
impl Membership {
    pub(crate) async fn lane_assignments(
        &self,
        store: &Store,
        keys: impl IntoIterator<Item = String>,
    ) -> Result<BTreeMap<String, String>, Error> {
        if self.control.is_draining() {
            return Err(Error::Lost);
        }
        let mut capable = self
            .capabilities(store)
            .await?
            .into_iter()
            .filter(|m| m.delayed_lane && !m.draining)
            .map(|m| m.instance)
            .collect::<Vec<_>>();
        // Mongo natural order is not a placement contract. Match membership's
        // canonical instance ordering so independent readers choose identically.
        capable.sort();
        capable.dedup();
        if capable.is_empty() {
            return Ok(BTreeMap::new());
        }
        balanced(keys, &capable)
    }
}

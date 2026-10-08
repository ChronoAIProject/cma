use crate::ingress::launch::LaunchRequest;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
pub(super) fn render(request: &LaunchRequest) -> BTreeMap<String, String> {
    let hash = |value: &str| format!("{:x}", Sha256::digest(value.as_bytes()))[..16].to_owned();
    let mut values = BTreeMap::from([
        ("cmaeg.channel".into(), hash(request.channel.id.as_str())),
        ("cmaeg.event".into(), request.activity.event_id.to_string()),
        (
            "cmaeg.partition".into(),
            hash(
                &request
                    .activity
                    .partition_key(&request.channel.definition.partition),
            ),
        ),
        (
            "cmaeg.actor".into(),
            if request.activity.actor.id.len() <= 512 {
                request.activity.actor.id.clone()
            } else {
                format!(
                    "sha256:{:x}",
                    Sha256::digest(request.activity.actor.id.as_bytes())
                )
            },
        ),
    ]);
    if request.activity.routing.is_some() {
        values.insert(
            "cmaeg.consumer".into(),
            request
                .consumer
                .as_ref()
                .map_or("default", cmaeg_core::ids::ConsumerId::as_str)
                .to_owned(),
        );
    }
    if request.event_ref.as_str().len() <= 512 {
        values.insert(
            "cmaeg.event_ref".into(),
            request.event_ref.as_str().to_owned(),
        );
    } else {
        tracing::warn!(
            "event reference exceeds provider metadata limit; context lookup is required"
        );
    }
    values
}

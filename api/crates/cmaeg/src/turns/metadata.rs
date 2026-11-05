use crate::ingress::launch::LaunchRequest;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
pub(crate) fn render(request: &LaunchRequest) -> BTreeMap<String, String> {
    render_for_partition(
        request,
        &request
            .activity
            .partition_key(&request.channel.definition.partition),
    )
}
pub(super) fn render_for_partition(
    request: &LaunchRequest,
    partition: &str,
) -> BTreeMap<String, String> {
    let hash = |value: &str| format!("{:x}", Sha256::digest(value.as_bytes()))[..16].to_owned();
    let mut values = BTreeMap::from([
        ("cmaeg.channel".into(), hash(request.channel.id.as_str())),
        ("cmaeg.event".into(), request.activity.event_id.to_string()),
        ("cmaeg.partition".into(), hash(partition)),
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

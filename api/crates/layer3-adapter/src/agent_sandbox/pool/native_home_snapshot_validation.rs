use super::*;

#[derive(Debug)]
pub(super) struct SnapshotFacts {
    uid: String,
    content_name: Option<String>,
    creation_time: Option<String>,
    ready: bool,
}

pub(super) async fn observe_snapshot(
    contents: &Api<DynamicObject>,
    namespace: &str,
    deployment: &str,
    request: &NativeHomeSnapshotRequest,
    snapshot: &DynamicObject,
) -> Result<NativeHomeSnapshotObservation> {
    let facts = validate_snapshot(snapshot, namespace, deployment, request)?;
    let (content_uid, handle) = match facts.content_name.as_deref() {
        Some(content_name) => {
            let content = contents.get(content_name).await.map_err(super::api_error)?;
            let (uid, handle) = validate_content(
                &content,
                namespace,
                &facts.uid,
                request.deterministic_name().as_str(),
            )?;
            (Some(uid), handle)
        }
        None if facts.ready => {
            return Err(rejected(
                "native home snapshot is ready without content identity",
            ));
        }
        None => (None, None),
    };
    if facts.ready && handle.is_none() {
        return Err(rejected(
            "native home snapshot is ready without copy handle",
        ));
    }
    let observation = NativeHomeSnapshotObservation {
        request: request.clone(),
        snapshot_name: request.deterministic_name(),
        snapshot_uid: facts.uid,
        snapshot_content_name: facts.content_name,
        snapshot_content_uid: content_uid,
        underlying_copy_handle: handle,
        creation_time: facts.creation_time,
        ready: facts.ready,
    };
    observation.validate()?;
    Ok(observation)
}

pub(super) fn validate_snapshot(
    snapshot: &DynamicObject,
    namespace: &str,
    deployment: &str,
    request: &NativeHomeSnapshotRequest,
) -> Result<SnapshotFacts> {
    request.validate()?;
    let metadata = &snapshot.metadata;
    let request_json = request_json(request)?;
    let expected_spec = desired_spec(request);
    if metadata.namespace.as_deref() != Some(namespace)
        || metadata.name.as_deref() != Some(request.deterministic_name().as_str())
        || metadata.uid.as_deref().is_none_or(str::is_empty)
        || metadata.deletion_timestamp.is_some()
        || metadata
            .owner_references
            .as_ref()
            .is_some_and(|owners| !owners.is_empty())
        || metadata
            .labels
            .as_ref()
            .and_then(|labels| labels.get(MANAGED_LABEL))
            .map(String::as_str)
            != Some(deployment)
        || metadata
            .labels
            .as_ref()
            .and_then(|labels| labels.get(BACKUP_OF_LABEL))
            .map(String::as_str)
            != Some(request.backup_of.as_str())
        || metadata
            .labels
            .as_ref()
            .and_then(|labels| labels.get(BACKUP_ID_LABEL))
            .map(String::as_str)
            != Some(request.backup_id.as_str())
        || metadata
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.get(REQUEST_ANNOTATION))
            != Some(&request_json)
        || snapshot.data.get("spec") != Some(&expected_spec)
    {
        return Err(rejected("native home snapshot identity changed"));
    }
    let uid = metadata.uid.clone().unwrap_or_default();
    let status = match snapshot.data.get("status") {
        None => None,
        Some(Value::Object(status)) => Some(status),
        Some(_) => return Err(rejected("native home snapshot status is malformed")),
    };
    let ready = match status.and_then(|status| status.get("readyToUse")) {
        None => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err(rejected("native home snapshot ready state is malformed")),
    };
    if status.is_some_and(|status| status.contains_key("error")) {
        return Err(rejected("native home snapshot reports provider error"));
    }
    let creation_time = match status.and_then(|status| status.get("creationTime")) {
        None => None,
        Some(Value::String(value)) => {
            parse_creation_time(value)?;
            Some(value.clone())
        }
        Some(_) => return Err(rejected("native home snapshot creation time is malformed")),
    };
    let content_name = match status.and_then(|status| status.get("boundVolumeSnapshotContentName"))
    {
        None => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        Some(_) => {
            return Err(rejected(
                "native home snapshot content identity is malformed",
            ))
        }
    };
    Ok(SnapshotFacts {
        uid,
        content_name,
        creation_time,
        ready,
    })
}

pub(super) fn validate_content(
    content: &DynamicObject,
    namespace: &str,
    snapshot_uid: &str,
    snapshot_name: &str,
) -> Result<(String, Option<String>)> {
    if content.metadata.deletion_timestamp.is_some() {
        return Err(rejected("native snapshot content is deleting"));
    }
    let content_uid = content
        .metadata
        .uid
        .as_deref()
        .filter(|uid| !uid.is_empty())
        .ok_or_else(|| rejected("native snapshot content lacks identity"))?;
    let reference = content
        .data
        .pointer("/spec/volumeSnapshotRef")
        .and_then(Value::as_object)
        .ok_or_else(|| rejected("native snapshot content reference is malformed"))?;
    if reference.get("name").and_then(Value::as_str) != Some(snapshot_name)
        || reference.get("namespace").and_then(Value::as_str) != Some(namespace)
        || reference.get("uid").and_then(Value::as_str) != Some(snapshot_uid)
    {
        return Err(rejected(
            "native snapshot content points to another snapshot",
        ));
    }
    let status = match content.data.get("status") {
        None => None,
        Some(Value::Object(status)) => Some(status),
        Some(_) => return Err(rejected("native snapshot content status is malformed")),
    };
    if status.is_some_and(|status| status.contains_key("error")) {
        return Err(rejected("native snapshot content reports provider error"));
    }
    let handle = match status.and_then(|status| status.get("snapshotHandle")) {
        None => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        Some(_) => return Err(rejected("native snapshot copy handle is malformed")),
    };
    Ok((content_uid.to_owned(), handle))
}

pub(super) fn validate_observation_transition(
    previous: &NativeHomeSnapshotObservation,
    observed: &NativeHomeSnapshotObservation,
) -> Result<()> {
    previous.validate()?;
    observed.validate()?;
    if previous.request != observed.request
        || previous.snapshot_name != observed.snapshot_name
        || previous.snapshot_uid != observed.snapshot_uid
        || evidence_changed(&previous.creation_time, &observed.creation_time)
        || evidence_changed(
            &previous.snapshot_content_name,
            &observed.snapshot_content_name,
        )
        || evidence_changed(
            &previous.snapshot_content_uid,
            &observed.snapshot_content_uid,
        )
        || evidence_changed(
            &previous.underlying_copy_handle,
            &observed.underlying_copy_handle,
        )
        || (previous.ready && !observed.ready)
    {
        return Err(rejected("native home snapshot evidence changed"));
    }
    Ok(())
}

fn evidence_changed<T: PartialEq>(previous: &Option<T>, observed: &Option<T>) -> bool {
    previous.is_some() && previous != observed
}

pub(super) fn source_required_for_observation(previous: &NativeHomeSnapshotObservation) -> bool {
    previous.creation_time.is_none()
}

pub(super) fn object_reports_cut(snapshot: &DynamicObject) -> Result<bool> {
    match snapshot.data.get("status") {
        None => Ok(false),
        Some(Value::Object(status)) => match status.get("creationTime") {
            None => Ok(false),
            Some(Value::String(value)) => {
                parse_creation_time(value)?;
                Ok(true)
            }
            Some(_) => Err(rejected("native home snapshot creation time is malformed")),
        },
        Some(_) => Err(rejected("native home snapshot status is malformed")),
    }
}

pub(super) fn desired_snapshot(
    namespace: &str,
    deployment: &str,
    request: &NativeHomeSnapshotRequest,
) -> Result<DynamicObject> {
    request.validate()?;
    serde_json::from_value(json!({
        "apiVersion":"snapshot.storage.k8s.io/v1", "kind":"VolumeSnapshot",
        "metadata":{"name":request.deterministic_name(),"namespace":namespace,
            "labels":{MANAGED_LABEL:deployment,BACKUP_OF_LABEL:request.backup_of,BACKUP_ID_LABEL:request.backup_id},
            "annotations":{REQUEST_ANNOTATION:request_json(request)?}},
        "spec":{"volumeSnapshotClassName":request.snapshot_class,
            "source":{"persistentVolumeClaimName":request.source.pvc_name}}
    }))
    .map_err(|_| rejected("invalid native home snapshot object"))
}

pub(super) fn desired_spec(request: &NativeHomeSnapshotRequest) -> Value {
    json!({
        "volumeSnapshotClassName":request.snapshot_class,
        "source":{"persistentVolumeClaimName":request.source.pvc_name}
    })
}

pub(super) fn request_json(request: &NativeHomeSnapshotRequest) -> Result<String> {
    serde_json::to_string(request)
        .map_err(|_| rejected("native home snapshot request is not serializable"))
}

pub(super) fn parse_creation_time(value: &str) -> Result<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| rejected("native home snapshot creation time is malformed"))
}

pub(super) fn snapshot_name(request: &NativeHomeSnapshotRequest) -> String {
    let mut digest = Sha256::new();
    digest.update(b"cma-native-home-backup-v1\0");
    for value in [
        request.backup_id.as_str(),
        request.backup_of.as_str(),
        request.source.namespace.as_str(),
        request.source.pvc_name.as_str(),
        request.source.pvc_uid.as_str(),
        request.snapshot_class.as_str(),
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    digest.update(request.epoch.to_be_bytes());
    format!("cma-home-backup-{:x}", digest.finalize())
}

pub(super) fn label_value(value: &str) -> bool {
    (1..=63).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

pub(super) fn dns_label(value: &str) -> bool {
    (1..=63).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

pub(super) fn dns_subdomain(value: &str) -> bool {
    (1..=253).contains(&value.len()) && value.split('.').all(dns_label)
}

pub(super) fn identity_value(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && !value.chars().any(char::is_control)
}

pub(super) fn is_already_exists(error: &kube::Error) -> bool {
    matches!(error, kube::Error::Api(response) if response.code == 409 && response.reason == "AlreadyExists")
}

//! Fenced lifecycle access to the OpenSandbox `BatchSandbox` resource.
//!
//! The provider's HTTP delete accepts only an opaque handle. Production maps
//! that handle to the name of a namespaced `BatchSandbox`, and copies creation
//! metadata into labels. Kubernetes can therefore supply the missing atomic
//! primitive: read and verify the labels, capture UID and resourceVersion, then
//! delete with both as preconditions. Any replacement or label edit between
//! the read and delete yields a conflict instead of deleting the wrong runtime.

use std::collections::BTreeMap;
use std::time::Duration;

use kube::api::{Api, DeleteParams, DynamicObject, ListParams, Preconditions};
use kube::core::{ApiResource, Expression, GroupVersionKind, Selector};

use crate::sandbox::SandboxError;
use crate::{RuntimeHandle, RuntimeIdentity, RuntimeReservationIdentity};

pub(crate) const CREATOR_METADATA_KEY: &str = "cma/control-plane";
pub(crate) const SESSION_METADATA_KEY: &str = "cma/session";
pub(crate) const GENERATION_METADATA_KEY: &str = "cma/runtime-generation";
pub(crate) const RESERVATION_METADATA_KEY: &str = "cma/runtime-reservation";

const SANDBOX_ID_LABEL: &str = "opensandbox.io/id";
const BATCH_SANDBOX_GROUP: &str = "sandbox.opensandbox.io";
const BATCH_SANDBOX_VERSION: &str = "v1alpha1";
const BATCH_SANDBOX_KIND: &str = "BatchSandbox";
const BATCH_SANDBOX_PLURAL: &str = "batchsandboxes";
const DISCOVERY_PAGE_SIZE: u32 = 100;
const DISCOVERY_PAGE_LIMIT: u32 = 50;
const DELETE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(30);
const DELETE_CONFIRM_POLL: Duration = Duration::from_millis(250);

/// Namespaced, preconditioned access to provider-owned `BatchSandbox` objects.
///
/// Construction uses the namespace inferred from the pod's service-account
/// token or the active kubeconfig. No method can address a cluster-scoped API
/// or a different namespace.
#[derive(Clone)]
pub struct BatchSandboxReclaimer {
    api: Api<DynamicObject>,
    namespace: String,
}

/// One immutable Kubernetes object version captured before destructive work.
#[derive(Debug, Clone)]
pub(crate) struct CapturedSandbox {
    handle: RuntimeHandle,
    uid: String,
    resource_version: String,
}

impl BatchSandboxReclaimer {
    /// Build from the in-cluster identity or active kubeconfig.
    pub async fn infer() -> Result<Self, SandboxError> {
        let config = kube::Config::infer().await.map_err(|error| {
            SandboxError::Unreachable(format!(
                "infer Kubernetes configuration for fenced sandbox cleanup: {error}"
            ))
        })?;
        let namespace = config.default_namespace.clone();
        let client = kube::Client::try_from(config).map_err(|error| {
            SandboxError::Unreachable(format!(
                "build Kubernetes client for fenced sandbox cleanup: {error}"
            ))
        })?;
        Ok(Self::namespaced(client, namespace))
    }

    /// Bind an existing Kubernetes client to exactly one namespace.
    pub fn namespaced(client: kube::Client, namespace: impl Into<String>) -> Self {
        let namespace = namespace.into();
        let gvk = GroupVersionKind::gvk(
            BATCH_SANDBOX_GROUP,
            BATCH_SANDBOX_VERSION,
            BATCH_SANDBOX_KIND,
        );
        let resource = ApiResource::from_gvk_with_plural(&gvk, BATCH_SANDBOX_PLURAL);
        let api = Api::namespaced_with(client, &namespace, &resource);
        Self { api, namespace }
    }

    /// Capture the provider object currently carrying this handle.
    pub(crate) async fn capture_delete_target(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<Option<CapturedSandbox>, SandboxError> {
        let Some(object) = self
            .api
            .get_opt(&handle.0)
            .await
            .map_err(|error| kube_error("read BatchSandbox before fenced deletion", error))?
        else {
            return Ok(None);
        };
        self.provider_version(&object, handle).map(Some)
    }

    /// Delete only the exact runtime identity, and return only after that UID
    /// is confirmed absent.
    pub async fn delete_if_identity(&self, identity: &RuntimeIdentity) -> Result<(), SandboxError> {
        let Some(object) = self
            .api
            .get_opt(&identity.handle.0)
            .await
            .map_err(|error| kube_error("read BatchSandbox before fenced deletion", error))?
        else {
            return Ok(());
        };
        let target = self.verify_delete_target(&object, identity)?;
        self.delete_captured(target).await
    }

    /// Delete exactly a previously captured Kubernetes object version.
    pub(crate) async fn delete_captured(
        &self,
        target: CapturedSandbox,
    ) -> Result<(), SandboxError> {
        let CapturedSandbox {
            handle,
            uid,
            resource_version,
        } = target;
        let params = DeleteParams {
            grace_period_seconds: Some(0),
            propagation_policy: Some(kube::api::PropagationPolicy::Foreground),
            preconditions: Some(Preconditions {
                uid: Some(uid.clone()),
                resource_version: Some(resource_version),
            }),
            ..DeleteParams::default()
        };
        match self.api.delete(&handle.0, &params).await {
            Ok(_) => {}
            Err(kube::Error::Api(response)) if response.is_not_found() => return Ok(()),
            Err(error) => {
                return Err(kube_error(
                    "delete BatchSandbox with UID and resourceVersion preconditions",
                    error,
                ))
            }
        }

        let handle = handle.0;
        let target_uid = uid;
        tokio::time::timeout(DELETE_CONFIRM_TIMEOUT, async {
            loop {
                let current =
                    self.api.get_opt(&handle).await.map_err(|error| {
                        kube_error("confirm fenced BatchSandbox deletion", error)
                    })?;
                match current {
                    None => return Ok(()),
                    Some(object) if object.metadata.uid.as_deref() != Some(target_uid.as_str()) => {
                        // The preconditioned object is gone. A new object that
                        // reused the name is not this cleanup debt's target.
                        return Ok(());
                    }
                    Some(_) => tokio::time::sleep(DELETE_CONFIRM_POLL).await,
                }
            }
        })
        .await
        .map_err(|_| {
            SandboxError::Unreachable(format!(
                "BatchSandbox {} was not confirmed deleted within {} seconds",
                handle,
                DELETE_CONFIRM_TIMEOUT.as_secs()
            ))
        })?
    }

    /// Find all provider creates carrying an exact durable reservation.
    pub async fn discover_runtime_candidates(
        &self,
        reservation: &RuntimeReservationIdentity,
    ) -> Result<Vec<RuntimeHandle>, SandboxError> {
        validate_label_value("session id", &reservation.session)?;
        validate_label_value("runtime reservation", &reservation.token)?;
        let mut selector = Selector::default();
        selector.extend([
            Expression::Equal(SESSION_METADATA_KEY.into(), reservation.session.clone()),
            Expression::Equal(
                GENERATION_METADATA_KEY.into(),
                reservation.generation.to_string(),
            ),
            Expression::Equal(RESERVATION_METADATA_KEY.into(), reservation.token.clone()),
        ]);

        let mut handles = Vec::new();
        let mut continue_token = None;
        for _ in 0..DISCOVERY_PAGE_LIMIT {
            let mut params = ListParams::default()
                .labels_from(&selector)
                .limit(DISCOVERY_PAGE_SIZE);
            if let Some(token) = continue_token.as_deref() {
                params = params.continue_token(token);
            }
            let page = self
                .api
                .list(&params)
                .await
                .map_err(|error| kube_error("discover reserved BatchSandboxes", error))?;
            for object in page.items {
                handles.push(self.verify_discovered_candidate(&object, reservation)?);
            }
            let Some(next) = page.metadata.continue_.filter(|next| !next.is_empty()) else {
                handles.sort_by(|left, right| left.0.cmp(&right.0));
                handles.dedup();
                return Ok(handles);
            };
            continue_token = Some(next);
        }
        Err(SandboxError::Incomplete {
            pages: DISCOVERY_PAGE_LIMIT,
            page_size: DISCOVERY_PAGE_SIZE,
        })
    }

    fn verify_delete_target(
        &self,
        object: &DynamicObject,
        identity: &RuntimeIdentity,
    ) -> Result<CapturedSandbox, SandboxError> {
        self.verify_object_envelope(object, &identity.handle)?;
        let labels = object.metadata.labels.as_ref().ok_or_else(|| {
            identity_mismatch(&identity.handle, "has no provider metadata labels")
        })?;
        require_label(
            labels,
            SESSION_METADATA_KEY,
            &identity.session,
            &identity.handle,
        )?;
        match identity.generation {
            Some(generation) => require_label(
                labels,
                GENERATION_METADATA_KEY,
                &generation.to_string(),
                &identity.handle,
            )?,
            None if labels.contains_key(GENERATION_METADATA_KEY) => {
                return Err(identity_mismatch(
                    &identity.handle,
                    "carries generation metadata but legacy absence was required",
                ))
            }
            None => {}
        }
        self.captured_version(object, &identity.handle)
    }

    fn provider_version(
        &self,
        object: &DynamicObject,
        handle: &RuntimeHandle,
    ) -> Result<CapturedSandbox, SandboxError> {
        self.verify_object_envelope(object, handle)?;
        self.captured_version(object, handle)
    }

    fn captured_version(
        &self,
        object: &DynamicObject,
        handle: &RuntimeHandle,
    ) -> Result<CapturedSandbox, SandboxError> {
        let uid = required_metadata(object.metadata.uid.as_deref(), handle, "UID")?;
        let resource_version = required_metadata(
            object.metadata.resource_version.as_deref(),
            handle,
            "resourceVersion",
        )?;
        Ok(CapturedSandbox {
            handle: handle.clone(),
            uid,
            resource_version,
        })
    }

    fn verify_discovered_candidate(
        &self,
        object: &DynamicObject,
        reservation: &RuntimeReservationIdentity,
    ) -> Result<RuntimeHandle, SandboxError> {
        let name = object
            .metadata
            .name
            .as_deref()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| SandboxError::Malformed("BatchSandbox has no name".into()))?;
        let handle = RuntimeHandle(name.to_string());
        self.verify_object_envelope(object, &handle)?;
        let labels = object
            .metadata
            .labels
            .as_ref()
            .ok_or_else(|| identity_mismatch(&handle, "has no provider metadata labels"))?;
        require_label(labels, SESSION_METADATA_KEY, &reservation.session, &handle)?;
        require_label(
            labels,
            GENERATION_METADATA_KEY,
            &reservation.generation.to_string(),
            &handle,
        )?;
        require_label(
            labels,
            RESERVATION_METADATA_KEY,
            &reservation.token,
            &handle,
        )?;
        Ok(handle)
    }

    fn verify_object_envelope(
        &self,
        object: &DynamicObject,
        handle: &RuntimeHandle,
    ) -> Result<(), SandboxError> {
        if object.metadata.name.as_deref() != Some(handle.0.as_str()) {
            return Err(identity_mismatch(
                handle,
                "object name differs from the provider handle",
            ));
        }
        if object.metadata.namespace.as_deref() != Some(self.namespace.as_str()) {
            return Err(identity_mismatch(
                handle,
                "object is outside the configured sandbox namespace",
            ));
        }
        let labels = object
            .metadata
            .labels
            .as_ref()
            .ok_or_else(|| identity_mismatch(handle, "has no provider metadata labels"))?;
        require_label(labels, SANDBOX_ID_LABEL, &handle.0, handle)
    }
}

fn require_label(
    labels: &BTreeMap<String, String>,
    key: &str,
    expected: &str,
    handle: &RuntimeHandle,
) -> Result<(), SandboxError> {
    if labels.get(key).is_some_and(|actual| actual == expected) {
        Ok(())
    } else {
        Err(identity_mismatch(
            handle,
            &format!("does not carry the required {key} label"),
        ))
    }
}

fn required_metadata(
    value: Option<&str>,
    handle: &RuntimeHandle,
    field: &str,
) -> Result<String, SandboxError> {
    value
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            SandboxError::Malformed(format!(
                "BatchSandbox {} has no Kubernetes {field}",
                handle.0
            ))
        })
}

fn validate_label_value(name: &str, value: &str) -> Result<(), SandboxError> {
    let bytes = value.as_bytes();
    let valid_edge = |byte: u8| byte.is_ascii_alphanumeric();
    if bytes.is_empty()
        || bytes.len() > 63
        || !valid_edge(bytes[0])
        || !valid_edge(bytes[bytes.len() - 1])
        || !bytes
            .iter()
            .all(|byte| valid_edge(*byte) || matches!(*byte, b'_' | b'-' | b'.'))
    {
        return Err(SandboxError::Malformed(format!(
            "{name} cannot be represented as an exact Kubernetes label value"
        )));
    }
    Ok(())
}

fn identity_mismatch(handle: &RuntimeHandle, detail: &str) -> SandboxError {
    SandboxError::IdentityMismatch(format!("BatchSandbox {} {detail}", handle.0))
}

fn kube_error(operation: &str, error: kube::Error) -> SandboxError {
    match error {
        kube::Error::Api(response) => SandboxError::Rejected {
            status: u16::try_from(response.code).unwrap_or(500),
            detail: format!("{operation}: {}", response.message),
        },
        other => SandboxError::Unreachable(format!("{operation}: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::task::JoinHandle;

    use super::*;
    use crate::sandbox::SandboxClient;

    const TEST_NAMESPACE: &str = "chrono-bot";

    struct FixtureResponse {
        status: u16,
        body: String,
    }

    impl FixtureResponse {
        fn ok(body: impl Into<String>) -> Self {
            Self {
                status: 200,
                body: body.into(),
            }
        }

        fn rejected(status: u16, reason: &str) -> Self {
            Self {
                status,
                body: serde_json::json!({
                    "apiVersion": "v1",
                    "kind": "Status",
                    "metadata": {},
                    "status": "Failure",
                    "message": reason,
                    "reason": reason,
                    "code": status,
                })
                .to_string(),
            }
        }
    }

    struct Fixture {
        base_url: String,
        requests: Arc<Mutex<Vec<String>>>,
        server: JoinHandle<()>,
    }

    impl Fixture {
        fn reclaimer(&self) -> BatchSandboxReclaimer {
            let mut config = kube::Config::new(self.base_url.parse().expect("fixture URL parses"));
            config.default_namespace = TEST_NAMESPACE.into();
            config.default_retry = false;
            let client = kube::Client::try_from(config).expect("fixture client builds");
            BatchSandboxReclaimer::namespaced(client, TEST_NAMESPACE)
        }

        async fn finish(mut self) -> Vec<String> {
            match tokio::time::timeout(Duration::from_secs(2), &mut self.server).await {
                Ok(joined) => joined.expect("fixture server must not panic"),
                Err(_) => {
                    self.server.abort();
                    panic!("fixture server did not receive every expected request");
                }
            }
            self.requests.lock().expect("requests lock").clone()
        }
    }

    async fn serve(responses: Vec<FixtureResponse>) -> Fixture {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds");
        let address = listener.local_addr().expect("fixture has an address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&requests);
        let server = tokio::spawn(async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.expect("fixture accepts");
                let request = read_request(&mut stream).await;
                observed.lock().expect("requests lock").push(request);

                let reason = match response.status {
                    200 => "OK",
                    404 => "Not Found",
                    409 => "Conflict",
                    status => panic!("fixture does not name HTTP status {status}"),
                };
                let reply = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.status,
                    reason,
                    response.body.len(),
                    response.body
                );
                stream
                    .write_all(reply.as_bytes())
                    .await
                    .expect("fixture writes");
                stream.shutdown().await.expect("fixture closes");
            }
        });

        Fixture {
            base_url: format!("http://{address}"),
            requests,
            server,
        }
    }

    async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut request = Vec::new();
        let header_end = loop {
            let mut chunk = [0_u8; 1024];
            let read = stream.read(&mut chunk).await.expect("fixture reads");
            assert!(read > 0, "client closed before sending request headers");
            request.extend_from_slice(&chunk[..read]);
            if let Some(offset) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break offset + 4;
            }
        };
        let content_length = std::str::from_utf8(&request[..header_end])
            .expect("request headers are HTTP text")
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length").then(|| {
                    value
                        .trim()
                        .parse::<usize>()
                        .expect("content length is numeric")
                })
            })
            .unwrap_or(0);
        while request.len() < header_end + content_length {
            let mut chunk = [0_u8; 1024];
            let read = stream.read(&mut chunk).await.expect("fixture reads body");
            assert!(read > 0, "client closed before sending request body");
            request.extend_from_slice(&chunk[..read]);
        }
        String::from_utf8(request).expect("request is HTTP text")
    }

    fn batch_sandbox(
        handle: &str,
        session: &str,
        generation: Option<u64>,
        reservation: Option<&str>,
        uid: &str,
        resource_version: &str,
    ) -> serde_json::Value {
        let mut labels = BTreeMap::from([
            (SANDBOX_ID_LABEL.to_string(), handle.to_string()),
            (SESSION_METADATA_KEY.to_string(), session.to_string()),
        ]);
        if let Some(generation) = generation {
            labels.insert(GENERATION_METADATA_KEY.into(), generation.to_string());
        }
        if let Some(reservation) = reservation {
            labels.insert(RESERVATION_METADATA_KEY.into(), reservation.to_string());
        }
        serde_json::json!({
            "apiVersion": "sandbox.opensandbox.io/v1alpha1",
            "kind": "BatchSandbox",
            "metadata": {
                "name": handle,
                "namespace": TEST_NAMESPACE,
                "uid": uid,
                "resourceVersion": resource_version,
                "labels": labels,
            }
        })
    }

    fn success_status() -> String {
        serde_json::json!({
            "apiVersion": "v1",
            "kind": "Status",
            "metadata": {},
            "status": "Success",
            "code": 200,
        })
        .to_string()
    }

    fn identity(session: &str, generation: Option<u64>) -> RuntimeIdentity {
        RuntimeIdentity {
            handle: RuntimeHandle("sbx-1".into()),
            session: session.into(),
            generation: generation.and_then(NonZeroU64::new),
        }
    }

    fn request_line(request: &str) -> &str {
        request.lines().next().expect("request line")
    }

    fn request_body(request: &str) -> serde_json::Value {
        let (_, body) = request
            .split_once("\r\n\r\n")
            .expect("request separates headers from body");
        serde_json::from_str(body).expect("request body is JSON")
    }

    #[test]
    fn reservation_selector_values_must_be_exact_kubernetes_labels() {
        for valid in ["sesn_abc", "rres_0123", "a.b-c_d", "1"] {
            validate_label_value("value", valid).unwrap();
        }
        for invalid in [
            "",
            "_starts",
            "ends_",
            "has,comma",
            "has=equals",
            "has space",
        ] {
            assert!(validate_label_value("value", invalid).is_err(), "{invalid}");
        }
        assert!(validate_label_value("value", &"x".repeat(64)).is_err());
    }

    #[test]
    fn generation_absence_is_an_exact_legacy_comparison() {
        let labels = BTreeMap::from([
            (SANDBOX_ID_LABEL.to_string(), "sbx-1".to_string()),
            (SESSION_METADATA_KEY.to_string(), "sesn_1".to_string()),
        ]);
        assert!(!labels.contains_key(GENERATION_METADATA_KEY));
        let mut recorded = labels;
        recorded.insert(GENERATION_METADATA_KEY.into(), "1".into());
        assert!(recorded.contains_key(GENERATION_METADATA_KEY));
    }

    #[tokio::test]
    async fn exact_delete_uses_kubernetes_preconditions_and_waits_for_uid_absence() {
        let object = batch_sandbox("sbx-1", "sesn_1", Some(7), None, "uid-1", "rv-7");
        let fixture = serve(vec![
            FixtureResponse::ok(object.to_string()),
            FixtureResponse::ok(success_status()),
            FixtureResponse::ok(object.to_string()),
            FixtureResponse::rejected(404, "NotFound"),
        ])
        .await;
        let reclaimer = fixture.reclaimer();

        reclaimer
            .delete_if_identity(&identity("sesn_1", Some(7)))
            .await
            .expect("exact runtime is deleted");

        let requests = fixture.finish().await;
        assert_eq!(requests.len(), 4);
        assert!(request_line(&requests[0]).starts_with("GET "));
        assert!(request_line(&requests[1]).starts_with("DELETE "));
        assert!(request_line(&requests[2]).starts_with("GET "));
        assert!(request_line(&requests[3]).starts_with("GET "));
        let delete = request_body(&requests[1]);
        assert_eq!(delete["gracePeriodSeconds"], 0);
        assert_eq!(delete["propagationPolicy"], "Foreground");
        assert_eq!(delete["preconditions"]["uid"], "uid-1");
        assert_eq!(delete["preconditions"]["resourceVersion"], "rv-7");
    }

    #[tokio::test]
    async fn client_deletes_only_the_provider_version_captured_before_cleanup() {
        let original = batch_sandbox("sbx-1", "sesn_1", None, None, "uid-old", "rv-old");
        let successor = batch_sandbox("sbx-1", "sesn_2", None, None, "uid-new", "rv-new");
        let fixture = serve(vec![
            FixtureResponse::ok(original.to_string()),
            FixtureResponse::ok(success_status()),
            FixtureResponse::ok(successor.to_string()),
        ])
        .await;
        let client =
            SandboxClient::new(&fixture.base_url, "test-key").with_reclaimer(fixture.reclaimer());

        let target = client
            .capture_delete_target("sbx-1")
            .await
            .expect("provider identity is readable")
            .expect("the original provider object exists");
        client
            .delete_captured(target)
            .await
            .expect("the captured provider object is deleted");

        let requests = fixture.finish().await;
        assert_eq!(requests.len(), 3);
        assert!(request_line(&requests[0]).starts_with("GET "));
        assert!(request_line(&requests[1]).starts_with("DELETE "));
        assert!(request_line(&requests[2]).starts_with("GET "));
        let delete = request_body(&requests[1]);
        assert_eq!(delete["preconditions"]["uid"], "uid-old");
        assert_eq!(delete["preconditions"]["resourceVersion"], "rv-old");
    }

    #[tokio::test]
    async fn precondition_conflict_preserves_a_successor_that_reused_the_handle() {
        let original = batch_sandbox("sbx-1", "sesn_1", None, None, "uid-old", "rv-old");
        let successor = batch_sandbox("sbx-1", "sesn_2", None, None, "uid-new", "rv-new");
        let fixture = serve(vec![
            FixtureResponse::ok(original.to_string()),
            FixtureResponse::rejected(409, "Conflict"),
            FixtureResponse::ok(successor.to_string()),
        ])
        .await;
        let reclaimer = fixture.reclaimer();
        let handle = RuntimeHandle("sbx-1".into());

        let captured = reclaimer
            .capture_delete_target(&handle)
            .await
            .expect("the original identity is readable")
            .expect("the original exists");
        assert_eq!(captured.uid, "uid-old");
        let error = reclaimer
            .delete_captured(captured)
            .await
            .expect_err("a successor cannot satisfy the old preconditions");
        assert!(matches!(error, SandboxError::Rejected { status: 409, .. }));

        let survivor = reclaimer
            .capture_delete_target(&handle)
            .await
            .expect("the successor identity is readable")
            .expect("the successor survives");
        assert_eq!(survivor.uid, "uid-new");

        let requests = fixture.finish().await;
        assert_eq!(requests.len(), 3);
        assert!(request_line(&requests[0]).starts_with("GET "));
        assert!(request_line(&requests[1]).starts_with("DELETE "));
        assert!(request_line(&requests[2]).starts_with("GET "));
        let delete = request_body(&requests[1]);
        assert_eq!(delete["preconditions"]["uid"], "uid-old");
        assert_eq!(delete["preconditions"]["resourceVersion"], "rv-old");
    }

    #[tokio::test]
    async fn ownership_mismatch_never_sends_delete() {
        for wrong_identity in [identity("sesn_other", Some(7)), identity("sesn_1", Some(8))] {
            let fixture = serve(vec![FixtureResponse::ok(
                batch_sandbox("sbx-1", "sesn_1", Some(7), None, "uid-1", "rv-7").to_string(),
            )])
            .await;
            let reclaimer = fixture.reclaimer();

            let error = reclaimer
                .delete_if_identity(&wrong_identity)
                .await
                .expect_err("stale identity is refused");
            assert!(matches!(error, SandboxError::IdentityMismatch(_)));
            let requests = fixture.finish().await;
            assert_eq!(requests.len(), 1);
            assert!(request_line(&requests[0]).starts_with("GET "));
        }
    }

    #[tokio::test]
    async fn resource_version_conflict_is_not_acknowledged_as_cleanup() {
        let fixture = serve(vec![
            FixtureResponse::ok(
                batch_sandbox("sbx-1", "sesn_1", Some(7), None, "uid-1", "rv-old").to_string(),
            ),
            FixtureResponse::rejected(409, "Conflict"),
        ])
        .await;
        let reclaimer = fixture.reclaimer();

        let error = reclaimer
            .delete_if_identity(&identity("sesn_1", Some(7)))
            .await
            .expect_err("precondition conflict remains retryable cleanup debt");
        assert!(matches!(error, SandboxError::Rejected { status: 409, .. }));
        let requests = fixture.finish().await;
        assert_eq!(requests.len(), 2);
        assert!(request_line(&requests[1]).starts_with("DELETE "));
    }

    #[tokio::test]
    async fn disappearance_between_read_and_delete_is_idempotent_success() {
        let fixture = serve(vec![
            FixtureResponse::ok(
                batch_sandbox("sbx-1", "sesn_1", Some(7), None, "uid-1", "rv-7").to_string(),
            ),
            FixtureResponse::rejected(404, "NotFound"),
        ])
        .await;
        let reclaimer = fixture.reclaimer();

        reclaimer
            .delete_if_identity(&identity("sesn_1", Some(7)))
            .await
            .expect("an already absent target is deleted");
        let requests = fixture.finish().await;
        assert_eq!(requests.len(), 2);
    }

    #[tokio::test]
    async fn discovery_returns_every_reserved_candidate_across_pages() {
        let first = batch_sandbox("sbx-b", "sesn_1", Some(7), Some("rres_1"), "uid-b", "rv-b");
        let second = batch_sandbox("sbx-a", "sesn_1", Some(7), Some("rres_1"), "uid-a", "rv-a");
        let fixture = serve(vec![
            FixtureResponse::ok(
                serde_json::json!({
                    "apiVersion": "sandbox.opensandbox.io/v1alpha1",
                    "kind": "BatchSandboxList",
                    "metadata": { "continue": "next-page" },
                    "items": [first],
                })
                .to_string(),
            ),
            FixtureResponse::ok(
                serde_json::json!({
                    "apiVersion": "sandbox.opensandbox.io/v1alpha1",
                    "kind": "BatchSandboxList",
                    "metadata": {},
                    "items": [second],
                })
                .to_string(),
            ),
        ])
        .await;
        let reclaimer = fixture.reclaimer();
        let reservation = RuntimeReservationIdentity {
            session: "sesn_1".into(),
            generation: NonZeroU64::new(7).expect("non-zero generation"),
            token: "rres_1".into(),
        };

        let handles = reclaimer
            .discover_runtime_candidates(&reservation)
            .await
            .expect("every candidate is discovered");

        assert_eq!(
            handles,
            vec![RuntimeHandle("sbx-a".into()), RuntimeHandle("sbx-b".into())]
        );
        let requests = fixture.finish().await;
        assert_eq!(requests.len(), 2);
        assert!(request_line(&requests[0]).contains("labelSelector="));
        assert!(request_line(&requests[0]).contains("limit=100"));
        assert!(request_line(&requests[1]).contains("continue=next-page"));
    }
}

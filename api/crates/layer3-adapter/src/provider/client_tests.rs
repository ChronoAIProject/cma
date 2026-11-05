use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::RuntimeHandle;

struct Reply {
    status: u16,
    body: &'static str,
}

struct Fixture {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn finish(mut self) -> Vec<String> {
        tokio::time::timeout(Duration::from_secs(2), &mut self.task)
            .await
            .expect("provider fixture received every request")
            .expect("provider fixture did not panic");
        self.requests.lock().expect("request lock").clone()
    }
}

async fn serve(replies: Vec<Reply>) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture binds");
    let address = listener.local_addr().expect("fixture address");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&requests);
    let task = tokio::spawn(async move {
        for reply in replies {
            let (mut stream, _) = listener.accept().await.expect("fixture accepts");
            let request = read_request(&mut stream).await;
            observed.lock().expect("request lock").push(request);
            let reason = match reply.status {
                200 => "OK",
                202 => "Accepted",
                204 => "No Content",
                status => panic!("fixture does not support status {status}"),
            };
            let response = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.status,
                reason,
                reply.body.len(),
                reply.body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("fixture writes");
            stream.shutdown().await.expect("fixture closes");
        }
    });
    Fixture {
        base_url: format!("http://{address}"),
        requests,
        task,
    }
}

async fn serve_raw(response: String) -> (String, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("raw fixture binds");
    let address = listener.local_addr().expect("raw fixture address");
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("raw fixture accepts");
        let request = read_request(&mut stream).await;
        stream
            .write_all(response.as_bytes())
            .await
            .expect("raw fixture writes");
        stream.shutdown().await.expect("raw fixture closes");
        request
    });
    (format!("http://{address}"), task)
}

pub(super) async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await.expect("fixture reads");
        assert!(read > 0, "client closed before request completed");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("content length"))
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await.expect("fixture reads body");
        assert!(read > 0, "client closed during request body");
        bytes.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8(bytes).expect("HTTP request is text")
}

fn provider_request() -> ProviderSandboxRequest {
    ProviderSandboxRequest {
        funded_until: None,
        image: ProviderImage {
            uri: "runtime:test".into(),
        },
        entrypoint: vec!["/runtime".into()],
        env: BTreeMap::from([("HOME".into(), "/home/cma".into())]),
        resource_limits: BTreeMap::from([
            ("cpu".into(), "1000m".into()),
            ("memory".into(), "1024Mi".into()),
        ]),
        timeout: None,
        metadata: BTreeMap::from([("cma/sandbox".into(), "sandbox-1".into())]),
        volumes: Vec::new(),
    }
}

#[test]
fn provider_rows_ignore_unknown_fields() {
    let sandbox: ProviderSandbox = serde_json::from_str(
        r#"{
            "id": "runtime-1",
            "status": {
                "state": "Running",
                "newProviderField": true
            },
            "newTopLevelField": 1
        }"#,
    )
    .expect("extended provider row remains readable");

    assert_eq!(sandbox.id, "runtime-1");
    assert!(sandbox.is_running());
}

#[tokio::test]
async fn provider_create_sends_exact_existing_mounts_and_refuses_implicit_allocation() {
    let fixture = serve(vec![Reply {
        status: 202,
        body: r#"{"id":"runtime-volume","status":{"state":"Running"}}"#,
    }])
    .await;
    let client = SandboxClient::new(&fixture.base_url, "fixture-provider");
    let mut request = provider_request();
    request.volumes.push(ProviderVolume {
        name: "home".into(),
        pvc: ProviderPersistentVolumeClaim {
            claim_name: "cma-home-test".into(),
            create_if_not_exists: false,
            delete_on_sandbox_termination: false,
        },
        mount_path: "/home/cma".into(),
        read_only: false,
        sub_path: Some("home".into()),
    });
    client.create(&request).await.unwrap();
    let sent = fixture.finish().await;
    let body: serde_json::Value =
        serde_json::from_str(sent[0].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        body["volumes"],
        serde_json::json!([{
        "name":"home", "pvc":{"claimName":"cma-home-test","createIfNotExists":false,"deleteOnSandboxTermination":false},
            "mountPath":"/home/cma", "readOnly":false, "subPath":"home"
        }])
    );
    request.volumes[0].pvc.create_if_not_exists = true;
    assert!(matches!(
        client.create(&request).await,
        Err(ProviderError::Malformed(_))
    ));
}

#[tokio::test]
async fn provider_create_receipt_hashes_actual_successful_response_bytes() {
    use sha2::{Digest, Sha256};
    const BODY: &str = "{ \"id\": \"runtime-response\", \"providerField\": true }\n";
    let fixture = serve(vec![Reply {
        status: 202,
        body: BODY,
    }])
    .await;
    let client = SandboxClient::new(&fixture.base_url, "fixture-provider");
    let (sandbox, response_sha256) = client.create_receipted(&provider_request()).await.unwrap();
    assert_eq!(sandbox.id, "runtime-response");
    assert_eq!(
        response_sha256,
        format!("{:x}", Sha256::digest(BODY.as_bytes()))
    );
    let sent = fixture.finish().await;
    assert_eq!(sent.len(), 1);
    assert!(sent[0].starts_with("POST /v1/sandboxes HTTP/1.1"));
}

#[tokio::test]
async fn provider_hash_label_encoding_is_reversible_on_create_get_and_list() {
    let original = format!("-{}", "a".repeat(42));
    let wire = format!("h.{original}");
    let row = serde_json::json!({ "id": "runtime-label", "metadata": { "cma/spec-sha256": wire, "cma/home-identity": wire } });
    for operation in ["create", "get", "list"] {
        let body = if operation == "list" {
            serde_json::json!({"items":[row]}).to_string()
        } else {
            row.to_string()
        };
        let (origin, task) = serve_raw(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())).await;
        let client = SandboxClient::new(origin, "fixture-provider");
        let mut request = provider_request();
        request
            .metadata
            .insert("cma/spec-sha256".into(), original.clone());
        request
            .metadata
            .insert("cma/home-identity".into(), original.clone());
        let received = match operation {
            "create" => client.create(&request).await.unwrap(),
            "get" => client.get("runtime-label").await.unwrap(),
            _ => client.list().await.unwrap().remove(0),
        };
        assert_eq!(received.metadata["cma/spec-sha256"], original);
        assert_eq!(request.metadata["cma/spec-sha256"], original);
        assert_eq!(received.metadata["cma/home-identity"], original);
        assert_eq!(request.metadata["cma/home-identity"], original);
        let observed = task.await.unwrap();
        if operation == "create" {
            let (_, body) = observed.split_once("\r\n\r\n").unwrap();
            let sent: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!(sent["metadata"]["cma/spec-sha256"], wire);
            assert_eq!(sent["metadata"]["cma/home-identity"], wire);
        }
    }
}

#[tokio::test]
async fn provider_lifecycle_uses_published_paths_and_header_authentication() {
    let fixture = serve(vec![
        Reply {
            status: 202,
            body: r#"{"id":"runtime-1","status":{"state":"Running"}}"#,
        },
        Reply {
            status: 200,
            body: r#"{"endpoint":"provider.test/proxy/3001"}"#,
        },
        Reply {
            status: 200,
            body: r#"{"items":[{"id":"runtime-1","status":{"state":"Running"}}],"pagination":{"page":1,"pageSize":100,"totalItems":1,"totalPages":1,"hasNextPage":false}}"#,
        },
        Reply {
            status: 204,
            body: "",
        },
    ])
    .await;
    let client = SandboxClient::new(&fixture.base_url, "provider-secret");

    let created = client
        .create(&provider_request())
        .await
        .expect("create succeeds");
    assert!(created.is_running());
    assert_eq!(
        client
            .endpoint("runtime-1", 3001)
            .await
            .expect("endpoint resolves")
            .endpoint,
        "provider.test/proxy/3001"
    );
    assert_eq!(client.list().await.expect("list succeeds").len(), 1);
    client
        .delete(&RuntimeHandle("runtime-1".into()))
        .await
        .expect("delete succeeds");

    let requests = fixture.finish().await;
    let first_lines: Vec<_> = requests
        .iter()
        .map(|request| request.lines().next().expect("request line"))
        .collect();
    assert_eq!(
        first_lines,
        [
            "POST /v1/sandboxes HTTP/1.1",
            "GET /v1/sandboxes/runtime-1/endpoints/3001?use_server_proxy=true HTTP/1.1",
            "GET /v1/sandboxes?page=1&pageSize=100 HTTP/1.1",
            "DELETE /v1/sandboxes/runtime-1 HTTP/1.1",
        ]
    );
    assert!(requests.iter().all(|request| request
        .to_ascii_lowercase()
        .contains("open-sandbox-api-key: provider-secret")));
    assert!(requests.iter().all(|request| !request
        .lines()
        .next()
        .unwrap_or_default()
        .contains("provider-secret")));

    let body = requests[0]
        .split_once("\r\n\r\n")
        .expect("create has body")
        .1;
    let json: serde_json::Value = serde_json::from_str(body).expect("create body is JSON");
    assert!(json["timeout"].is_null());
    assert_eq!(json["resourceLimits"]["cpu"], "1000m");
    assert!(json.get("resource_limits").is_none());
    assert!(json.get("volumes").is_none());
}

#[test]
fn provider_client_never_places_its_key_in_the_base_url() {
    let client = SandboxClient::new("https://provider.example/", "provider-secret");
    assert_eq!(client.base_url(), "https://provider.example");
    assert!(!client.base_url().contains("provider-secret"));
    assert_eq!(
        client.auth_header(),
        ("OPEN-SANDBOX-API-KEY", "provider-secret")
    );
}

#[tokio::test]
async fn provider_client_rejects_oversized_responses_before_reading_them() {
    let (base_url, request) = serve_raw(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        4 * 1024 * 1024 + 1
    ))
    .await;
    let error = SandboxClient::new(base_url, "provider-secret")
        .get("runtime-1")
        .await
        .expect_err("oversized provider response is refused");

    assert!(matches!(error, ProviderError::Malformed(detail)
        if detail.contains("size limit")));
    assert!(request
        .await
        .expect("raw fixture completes")
        .starts_with("GET /v1/sandboxes/runtime-1 HTTP/1.1"));
}

#[tokio::test]
async fn provider_redirects_are_not_followed_or_given_the_api_key() {
    let redirect_target = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("redirect target binds");
    let target = redirect_target
        .local_addr()
        .expect("redirect target address");
    let (base_url, request) = serve_raw(format!(
        "HTTP/1.1 302 Found\r\nLocation: http://{target}/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    ))
    .await;
    let error = SandboxClient::new(base_url, "provider-secret")
        .get("runtime-1")
        .await
        .expect_err("provider redirect is refused");

    assert!(matches!(error, ProviderError::Rejected { status: 302, .. }));
    let request = request.await.expect("redirect fixture completes");
    assert!(request
        .to_ascii_lowercase()
        .contains("open-sandbox-api-key: provider-secret"));
    assert!(
        tokio::time::timeout(Duration::from_millis(200), redirect_target.accept())
            .await
            .is_err(),
        "redirect target unexpectedly received the provider credential"
    );
}

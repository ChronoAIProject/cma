use super::*;
use k8s_openapi::api::core::v1::Pod;
use kube::{api::ListParams, Api};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn remote() -> RemoteKubernetesConfig {
    RemoteKubernetesConfig {
        server: "https://native-api.example".into(),
        ca_file: "/var/run/native/ca.crt".into(),
        auth: RemoteKubernetesAuth::TokenFile {
            path: "/var/run/native/token".into(),
        },
    }
}

#[test]
fn explicit_connection_has_only_pinned_tls_and_refreshable_file_auth() {
    let config = remote().configuration().unwrap();
    assert_eq!(
        config.cluster_url.to_string(),
        "https://native-api.example/"
    );
    assert_eq!(config.root_cert_file, Some("/var/run/native/ca.crt".into()));
    assert_eq!(
        config.auth_info.token_file.as_deref(),
        Some("/var/run/native/token")
    );
    assert!(!config.accept_invalid_certs);
    assert!(config.auth_info.exec.is_none());
    assert!(config.auth_info.auth_provider.is_none());
    assert!(config.auth_info.token.is_none());
    assert!(config.auth_info.impersonate.is_none());
    assert!(config.proxy_url.is_none());
}

#[test]
fn invalid_remote_targets_and_relative_credentials_are_rejected_without_echoing_input() {
    for server in [
        "http://native-api.example",
        "https://user:secret@native-api.example",
        "https://native-api.example/path",
        "https://native-api.example?token=secret",
        "https://native-api.example/",
        "https://NATIVE-api.example",
    ] {
        let error = RemoteKubernetesConfig {
            server: server.into(),
            ..remote()
        }
        .validate()
        .unwrap_err()
        .to_string();
        assert!(!error.contains(server));
        assert!(!error.contains("secret"));
    }
    assert!(RemoteKubernetesConfig {
        auth: RemoteKubernetesAuth::TokenFile {
            path: "token".into()
        },
        ..remote()
    }
    .validate()
    .is_err());
    assert!(RemoteKubernetesConfig {
        ca_file: String::new(),
        ..remote()
    }
    .validate()
    .is_err());
}

#[tokio::test]
async fn unavailable_remote_credentials_never_fall_back_to_local_auth() {
    let directory = std::env::temp_dir().join(format!("cma-absent-{}", uuid::Uuid::new_v4()));
    let config = RemoteKubernetesConfig {
        ca_file: directory.join("ca.crt").to_str().unwrap().into(),
        auth: RemoteKubernetesAuth::TokenFile {
            path: directory.join("private-token").to_str().unwrap().into(),
        },
        ..remote()
    };
    let error = match resolve_kubernetes_connection(Some(&config)).await {
        Ok(_) => panic!("missing credentials must refuse explicit native client"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("mounted credentials are unavailable"));
    assert!(!error.contains("private-token"));
    assert!(!error.contains(&config.server));
}

// Exercise separate real kube clients against isolated loopback APIs. Only this
// fixture substitutes HTTP for the independently validated production TLS config.
#[tokio::test]
async fn two_clients_keep_their_own_authority_namespace_and_token() {
    async fn endpoint() -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut buffer = [0; 4096];
                let length = socket.read(&mut buffer).await.unwrap();
                assert!(length > 0);
                bytes.extend_from_slice(&buffer[..length]);
                if bytes.windows(4).any(|v| v == b"\r\n\r\n") {
                    break;
                }
            }
            let body = r#"{"apiVersion":"v1","kind":"PodList","metadata":{},"items":[]}"#;
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            String::from_utf8(bytes).unwrap().to_ascii_lowercase()
        });
        (origin, task)
    }
    let (local_url, local_request) = endpoint().await;
    let (remote_url, remote_request) = endpoint().await;
    let token = std::env::temp_dir().join(format!("cma-native-token-{}", uuid::Uuid::new_v4()));
    std::fs::write(&token, b"native-fixture-value").unwrap();
    let mut local = kube::Config::new(local_url.parse().unwrap());
    local.auth_info.token = Some("local-fixture-value".into());
    let local = kube::Client::try_from(local).unwrap();
    let mut native = RemoteKubernetesConfig {
        auth: RemoteKubernetesAuth::TokenFile {
            path: token.to_str().unwrap().into(),
        },
        ..remote()
    }
    .configuration()
    .unwrap();
    native.cluster_url = remote_url.parse().unwrap();
    native.root_cert_file = None;
    let native = kube::Client::try_from(native).unwrap();
    let a: Api<Pod> = Api::namespaced(local, "retained-local");
    let b: Api<Pod> = Api::namespaced(native, "native-remote");
    let options = ListParams::default();
    let (a, b) = tokio::join!(a.list(&options), b.list(&options));
    a.unwrap();
    b.unwrap();
    let local = local_request.await.unwrap();
    let native = remote_request.await.unwrap();
    std::fs::remove_file(token).unwrap();
    assert!(local.contains("/api/v1/namespaces/retained-local/pods"));
    assert!(local.contains("authorization: bearer local-fixture-value"));
    assert!(!local.contains("native-fixture-value"));
    assert!(native.contains("/api/v1/namespaces/native-remote/pods"));
    assert!(native.contains("authorization: bearer native-fixture-value"));
    assert!(!native.contains("local-fixture-value"));
}

#[test]
fn metadata_mode_has_pinned_tls_and_never_infers_other_credentials() {
    let remote = RemoteKubernetesConfig {
        auth: RemoteKubernetesAuth::GoogleMetadata,
        ..remote()
    };
    let config = remote.configuration().unwrap();
    assert_eq!(config.root_cert_file, Some("/var/run/native/ca.crt".into()));
    assert!(config.auth_info.token_file.is_none() && config.auth_info.token.is_none());
    assert!(config.auth_info.exec.is_none() && config.auth_info.auth_provider.is_none());
    assert!(!config.accept_invalid_certs);
}

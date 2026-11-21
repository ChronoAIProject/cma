use super::*;
use serde_json::json;
use std::{
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const OP: &str = "de98646f-1234-4234-8234-0123456789ab";
struct Fixture {
    custody: GcsCustody,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
    path: std::path::PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}
async fn fixture(failure: &'static str) -> Fixture {
    let path = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("custody-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    for (name, value) in [("create", "create-only"), ("verify", "read-only")] {
        std::fs::write(path.join(name), value).unwrap();
        std::fs::set_permissions(path.join(name), std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = requests.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let n = socket.read(&mut buffer).await.unwrap();
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buffer[..n]);
                if data.windows(4).any(|x| x == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&data).to_string();
            observed.lock().unwrap().push(request.clone());
            let first = request.lines().next().unwrap();
            let writer = request.contains("Bearer create-only");
            if failure == "lost_reply" && first.starts_with("POST ") {
                drop(socket);
                continue;
            }
            let payload = if first.contains("testPermissions") {
                let mut p = vec![
                    "storage.buckets.get",
                    if writer {
                        "storage.objects.create"
                    } else {
                        "storage.objects.get"
                    },
                ];
                if failure == "destructive" {
                    p.push("storage.objects.delete");
                }
                serde_json::to_vec(&json!({"permissions":p})).unwrap()
            } else if first.starts_with("POST ") {
                b"{}".to_vec()
            } else if first.contains("alt=media") {
                if failure == "corrupt" {
                    b"broken".to_vec()
                } else {
                    b"archive".to_vec()
                }
            } else if first.contains("/o/") {
                serde_json::to_vec(&json!({"bucket":"archive-fixture","name":key(OP,b"archive").unwrap(),"generation":if failure=="generation" {"2"}else{"1"},"size":"7","eventBasedHold":failure!="hold"})).unwrap()
            } else {
                serde_json::to_vec(&json!({"name":"archive-fixture","labels":{"cma-retirement-operation":OP},"metageneration":"1","defaultEventBasedHold":true,
            "iamConfiguration":{"uniformBucketLevelAccess":{"enabled":true},"publicAccessPrevention":"enforced"},
            "lifecycle":{"rule":if failure=="expiry" {json!([{"action":{"type":"Delete"}}])}else{json!([])}}})).unwrap()
            };
            let status = if first.starts_with("POST ") {
                "412 Precondition Failed"
            } else {
                "200 OK"
            };
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            socket.write_all(header.as_bytes()).await.unwrap();
            socket.write_all(&payload).await.unwrap();
        }
    });
    let mut custody = GcsCustody::new(
        "archive-fixture".into(),
        OP.into(),
        Some(path.join("create")),
        path.join("verify"),
    )
    .unwrap();
    custody.test_origin(url::Url::parse(&format!("http://{address}/")).unwrap());
    Fixture {
        custody,
        requests,
        task,
        path,
    }
}
#[tokio::test]
async fn lost_upload_reply_reconciles_exact_generation_with_independent_reader() {
    let fixture = fixture("lost_reply").await;
    let receipt = fixture
        .custody
        .retain(OP, b"archive".to_vec())
        .await
        .unwrap();
    assert_eq!(fixture.custody.read(&receipt).await.unwrap(), b"archive");
    let requests = fixture.requests.lock().unwrap();
    assert!(requests.iter().any(|r| r.starts_with("POST ")
        && r.contains("ifGenerationMatch=0")
        && r.contains("Bearer create-only")));
    assert!(requests
        .iter()
        .filter(|r| r.contains("alt=media"))
        .all(|r| r.contains("generation=1") && r.contains("Bearer read-only")));
    assert!(!requests
        .iter()
        .any(|r| r.starts_with("DELETE ") || r.starts_with("PUT ") || r.starts_with("PATCH ")));
}
#[tokio::test]
async fn destructive_credentials_expiry_missing_hold_and_corrupt_bytes_refuse() {
    for reason in ["destructive", "expiry", "hold", "corrupt"] {
        let fixture = fixture(reason).await;
        assert!(
            fixture
                .custody
                .retain(OP, b"archive".to_vec())
                .await
                .is_err(),
            "{reason}"
        );
    }
}
#[tokio::test]
async fn wrong_generation_hash_or_operation_cannot_be_read() {
    let fixture = fixture("generation").await;
    let mut receipt = ObjectReceipt {
        key: key(OP, b"archive").unwrap(),
        generation: "1".into(),
        sha256: digest(b"archive"),
        bytes: 7,
    };
    assert!(fixture.custody.read(&receipt).await.is_err());
    receipt.generation = "2".into();
    receipt.sha256 = digest(b"other");
    assert!(fixture.custody.read(&receipt).await.is_err());
    assert!(fixture
        .custody
        .retain(&uuid::Uuid::new_v4().to_string(), b"archive".to_vec())
        .await
        .is_err());
}

#[tokio::test]
async fn independent_readback_needs_no_creation_credential() {
    let fixture = fixture("").await;
    let mut read_only = GcsCustody::new(
        "archive-fixture".into(),
        OP.into(),
        None,
        fixture.path.join("verify"),
    )
    .unwrap();
    // Both clients use this fixture origin; no actual provider is contacted.
    let seen = fixture.custody.policy().await.unwrap();
    assert!(seen.independent_read_only);
    // Refusal is before any network request even with a readable token file.
    assert!(read_only.retain(OP, b"archive".to_vec()).await.is_err());
    // Exercise independent policy/reads on the same mock server address.
    let address = fixture
        .requests
        .lock()
        .unwrap()
        .first()
        .unwrap()
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("host:"))
        .unwrap()
        .split_once(':')
        .unwrap()
        .1
        .trim()
        .to_string();
    read_only.test_origin(url::Url::parse(&format!("http://{address}/")).unwrap());
    assert_eq!(read_only.policy().await.unwrap(), seen);
    let receipt = ObjectReceipt {
        key: key(OP, b"archive").unwrap(),
        generation: "1".into(),
        sha256: digest(b"archive"),
        bytes: 7,
    };
    assert_eq!(read_only.read(&receipt).await.unwrap(), b"archive");
}

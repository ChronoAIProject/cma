use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use super::*;
use crate::{
    HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest, HomeVolumeScope,
};

fn reference(regional: bool) -> HomeBackingReference {
    let request = HomeVolumeRequest {
        volume_id: "home-original".into(),
        sandbox_id: "sandbox-original".into(),
        policy: HomeVolumePolicy {
            storage_class: "standard-rwo".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "seed-uid".into(),
        },
    };
    HomeBackingReference {
        provider_scope: HomeVolumeScope::from_authority(
            "kubernetes-pvc-v1",
            &["cluster", "namespace", "controller"],
        )
        .unwrap(),
        home: HomeVolumeAttachment {
            receipt: HomeVolumeReceipt {
                volume_id: request.volume_id.clone(),
                request_sha256: request.computed_sha256(),
                storage_reference: "original-pvc".into(),
                storage_generation: "original-pvc-uid".into(),
            },
            request,
        },
        pv_name: "original-pv".into(),
        pv_uid: "original-pv-uid".into(),
        csi_driver: "pd.csi.storage.gke.io".into(),
        volume_handle: if regional {
            "projects/test-project/regions/us-central1/disks/original-disk"
        } else {
            "projects/test-project/zones/us-central1-a/disks/original-disk"
        }
        .into(),
    }
}

fn disk(reference: &HomeBackingReference, generation: &str) -> Value {
    let path = disk_path(reference).unwrap();
    let mut value = json!({
        "kind": "compute#disk", "name": path.name, "id": generation,
        "selfLink": format!("https://www.googleapis.com/compute/v1/{}", path.handle),
    });
    value[path.location_field] = json!(format!(
        "https://www.googleapis.com/compute/v1/{}",
        path.location
    ));
    value
}

fn missing() -> Value {
    json!({"error": {"code": 404, "errors": [{"reason": "notFound"}]}})
}

fn token() -> (u16, String) {
    (
        200,
        json!({"access_token": "fixture-read-authority", "token_type": "Bearer", "expires_in": 60})
            .to_string(),
    )
}

struct Fixture {
    observer: GkePdBackingObserver,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn finish(self) -> Vec<String> {
        tokio::time::timeout(Duration::from_secs(3), self.task)
            .await
            .unwrap()
            .unwrap();
        self.requests.lock().unwrap().clone()
    }
}

async fn serve(replies: Vec<(u16, String)>) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = requests.clone();
    let task = tokio::spawn(async move {
        for (status, body) in replies {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = crate::provider::tests::read_request(&mut stream).await;
            observed.lock().unwrap().push(request);
            let response = format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nMetadata-Flavor: Google\r\nLocation: http://127.0.0.1:1/forbidden\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    Fixture {
        observer: GkePdBackingObserver {
            transport: transport::Transport::fixture(&origin).unwrap(),
        },
        requests,
        task,
    }
}

#[tokio::test]
async fn original_zonal_and_regional_disks_are_pinned_then_exact_absence_is_observed() {
    for regional in [false, true] {
        let reference = reference(regional);
        let fixture = serve(vec![
            token(),
            (200, disk(&reference, "123456789").to_string()),
            token(),
            (404, missing().to_string()),
        ])
        .await;
        let original = fixture.observer.capture_original(&reference).await.unwrap();
        assert_eq!(original.reference, reference);
        assert_eq!(original.backing_generation, "123456789");
        let absence = fixture
            .observer
            .observe_original_absence(&original)
            .await
            .unwrap();
        assert_eq!(absence.original, original);
        assert!(absence.request_id.starts_with("cma-pd-read-"));
        assert_eq!(absence.response_sha256.len(), 64);
        let requests = fixture.finish().await;
        assert_eq!(requests.len(), 4);
        for request in [&requests[0], &requests[2]] {
            assert!(request.starts_with("GET /token HTTP/1.1"));
            assert!(request.to_lowercase().contains("metadata-flavor: google"));
            assert!(!request.to_lowercase().contains("authorization:"));
        }
        for request in [&requests[1], &requests[3]] {
            assert!(request.starts_with(&format!(
                "GET /compute/v1/{} HTTP/1.1",
                reference.volume_handle
            )));
            assert!(request.contains("authorization: Bearer fixture-read-authority"));
            assert!(!request.contains("metadata-flavor:"));
        }
    }
}

#[tokio::test]
async fn present_or_replaced_disk_never_produces_an_absence_receipt() {
    let reference = reference(false);
    let original = HomeBackingIdentity {
        reference: reference.clone(),
        backing_generation: "123456789".into(),
    };
    for generation in ["123456789", "987654321"] {
        let fixture = serve(vec![
            token(),
            (200, disk(&reference, generation).to_string()),
        ])
        .await;
        assert!(fixture
            .observer
            .observe_original_absence(&original)
            .await
            .is_err());
        assert_eq!(fixture.finish().await.len(), 2);
    }
}

#[tokio::test]
async fn authorization_redirect_malformed_and_oversized_responses_stay_unresolved() {
    let reference = reference(false);
    let original = HomeBackingIdentity {
        reference,
        backing_generation: "123456789".into(),
    };
    for (status, body) in [
        (401, missing().to_string()),
        (403, missing().to_string()),
        (500, missing().to_string()),
        (302, missing().to_string()),
        (404, "<html>proxy not found</html>".into()),
        (
            404,
            json!({"error": {"code": 403, "errors": [{"reason": "notFound"}]}}).to_string(),
        ),
        (
            404,
            json!({"error": {"code": 404, "errors": [{"reason": "accessDenied"}]}}).to_string(),
        ),
        (404, " ".repeat(65_537)),
    ] {
        let fixture = serve(vec![token(), (status, body)]).await;
        assert!(fixture
            .observer
            .observe_original_absence(&original)
            .await
            .is_err());
        assert_eq!(fixture.finish().await.len(), 2);
    }
    for reply in [
        (302, token().1),
        (403, token().1),
        (
            200,
            json!({"access_token": "expired", "token_type": "Bearer", "expires_in": 0}).to_string(),
        ),
    ] {
        let fixture = serve(vec![reply]).await;
        assert!(fixture
            .observer
            .observe_original_absence(&original)
            .await
            .is_err());
        assert_eq!(fixture.finish().await.len(), 1);
    }
}

#[tokio::test]
async fn hostile_paths_and_unpinned_generations_are_refused_before_network_io() {
    let observer = GkePdBackingObserver {
        transport: transport::Transport::fixture("http://127.0.0.1:1").unwrap(),
    };
    let original = reference(false);
    for handle in [
        "https://evil.invalid/disks/a",
        "//evil.invalid/disks/a",
        "projects/test-project/zones/us-central1-a/disks/../foreign",
        "projects/test-project/zones/us-central1-a/disks/%2e%2e",
        "projects/test-project/zones/us-central1-a/disks/original-disk?alt=json",
        "projects/test-project/zones/us-central1-a/disks/original-disk#fragment",
        "projects/test-project/zones/us-central1-a/disks/original_disk",
        "projects/test-project/zones/us-central1-a/disks/original-disk/",
    ] {
        let mut bad = original.clone();
        bad.volume_handle = handle.into();
        assert!(matches!(
            observer.capture_original(&bad).await,
            Err(AdapterError::Rejected(_))
        ));
    }
    let mut wrong_driver = original.clone();
    wrong_driver.csi_driver = "another.csi.driver".into();
    assert!(matches!(
        observer.capture_original(&wrong_driver).await,
        Err(AdapterError::Rejected(_))
    ));
    for generation in ["", "0", "00123", "123/other", "18446744073709551616"] {
        let bad = HomeBackingIdentity {
            reference: original.clone(),
            backing_generation: generation.into(),
        };
        assert!(matches!(
            observer.observe_original_absence(&bad).await,
            Err(AdapterError::Rejected(_))
        ));
    }
}

#[tokio::test]
async fn original_capture_requires_provider_corroboration_of_path_location_and_generation() {
    let reference = reference(false);
    for (field, bad) in [
        ("id", json!("not-an-id")),
        ("id", json!(123)),
        ("kind", json!("compute#snapshot")),
        ("name", json!("other-disk")),
        (
            "selfLink",
            json!(
                "https://evil.invalid/compute/v1/projects/test-project/zones/us-central1-a/disks/original-disk"
            ),
        ),
        (
            "zone",
            json!(
                "https://www.googleapis.com/compute/v1/projects/test-project/zones/us-central1-b"
            ),
        ),
    ] {
        let mut value = disk(&reference, "123456789");
        value[field] = bad;
        let fixture = serve(vec![token(), (200, value.to_string())]).await;
        assert!(fixture.observer.capture_original(&reference).await.is_err());
        assert_eq!(fixture.finish().await.len(), 2);
    }
    let fixture = serve(vec![token(), (404, missing().to_string())]).await;
    assert!(fixture.observer.capture_original(&reference).await.is_err());
    assert_eq!(fixture.finish().await.len(), 2);
}

use super::super::{fixture::Fixture, tests::spec};
use super::*;
use std::{sync::Arc, time::Duration};

fn target(revision: i64, replicas: u32) -> NativePoolTarget {
    NativePoolTarget {
        target_id: format!("wp_{}", "a".repeat(64)),
        revision,
        replicas,
    }
}

fn retained_target(fixture: &Fixture, binding: &NativePoolBinding) -> NativePoolTarget {
    serde_json::from_str(
        fixture.row("sandboxwarmpools", &binding.name)["metadata"]["annotations"][ANNOTATION]
            .as_str()
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn paused_old_target_cannot_refresh_resource_version_and_overwrite_new_target() {
    let fixture = Fixture::new().await;
    let binding = fixture.client.configure(&spec(), 0).await.unwrap().binding;
    let started = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let first = AgentSandboxPools::new(
        fixture.kubernetes.clone(),
        super::super::super::tests::config(),
    );
    let original = binding.clone();
    let entered = started.clone();
    let release = resume.clone();
    let stale = tokio::spawn(async move {
        let original_target = target(1, 7);
        entered.notify_one();
        release.notified().await;
        // Resume with a fresh provider read but the ORIGINAL durable generation.
        first.resize_fenced(&original, &original_target).await
    });
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    fixture
        .client
        .resize_fenced(&binding, &target(2, 3))
        .await
        .unwrap();
    resume.notify_one();
    assert!(tokio::time::timeout(Duration::from_secs(2), stale)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    assert_eq!(retained_target(&fixture, &binding), target(2, 3));
    assert_eq!(
        fixture.row("sandboxwarmpools", &binding.name)["spec"]["replicas"],
        3
    );
}

#[tokio::test]
async fn lost_ack_replay_requires_exact_generation_identity_and_capacity() {
    let fixture = Fixture::new().await;
    let binding = fixture.client.configure(&spec(), 0).await.unwrap().binding;
    fixture.lost("PATCH", "sandboxwarmpools");
    fixture
        .client
        .resize_fenced(&binding, &target(4, 2))
        .await
        .unwrap();
    let before = fixture.row("sandboxwarmpools", &binding.name);
    fixture
        .client
        .resize_fenced(&binding, &target(4, 2))
        .await
        .unwrap();
    assert_eq!(fixture.row("sandboxwarmpools", &binding.name), before);
    for invalid in [
        target(3, 2),
        target(4, 9),
        NativePoolTarget {
            target_id: format!("wp_{}", "b".repeat(64)),
            ..target(5, 2)
        },
    ] {
        assert!(fixture
            .client
            .resize_fenced(&binding, &invalid)
            .await
            .is_err());
    }
    assert_eq!(fixture.row("sandboxwarmpools", &binding.name), before);
    assert!(fixture.client.resize(&binding, 8).await.is_err());
    assert!(fixture.client.configure(&spec(), 0).await.is_err());
    assert_eq!(retained_target(&fixture, &binding), target(4, 2));
}

#[tokio::test]
async fn target_cas_conflict_is_not_retried_with_new_receiver_version() {
    let fixture = Fixture::new().await;
    let binding = fixture.client.configure(&spec(), 0).await.unwrap().binding;
    fixture.state.lock().unwrap().conflict.push(format!(
        "PATCH {}/sandboxwarmpools/{}",
        super::super::fixture::prefix(),
        binding.name
    ));
    assert!(fixture
        .client
        .resize_fenced(&binding, &target(1, 4))
        .await
        .is_err());
    let row = fixture.row("sandboxwarmpools", &binding.name);
    assert_eq!(row["spec"]["replicas"], 0);
    assert!(row["metadata"]["annotations"].get(ANNOTATION).is_none());
    // A caller may retry the same durable generation; it must not invent one.
    fixture
        .client
        .resize_fenced(&binding, &target(1, 4))
        .await
        .unwrap();
    assert_eq!(retained_target(&fixture, &binding), target(1, 4));
}

#[tokio::test]
async fn target_fence_refuses_uid_recreation_malformed_watermark_and_unowned_drift() {
    for corruption in ["uid", "watermark", "capacity"] {
        let fixture = Fixture::new().await;
        let binding = fixture.client.configure(&spec(), 0).await.unwrap().binding;
        fixture
            .client
            .resize_fenced(&binding, &target(1, 3))
            .await
            .unwrap();
        fixture.edit("sandboxwarmpools", &binding.name, |row| match corruption {
            "uid" => row["metadata"]["uid"] = json!("replacement-uid"),
            "watermark" => row["metadata"]["annotations"][ANNOTATION] = json!("invalid"),
            _ => row["spec"]["replicas"] = json!(8),
        });
        let before = fixture.row("sandboxwarmpools", &binding.name);
        assert!(
            fixture
                .client
                .resize_fenced(&binding, &target(2, 5))
                .await
                .is_err(),
            "{corruption}"
        );
        assert_eq!(fixture.row("sandboxwarmpools", &binding.name), before);
    }
}

#[tokio::test]
async fn permanent_retirement_can_drain_without_resetting_target_watermark() {
    let fixture = Fixture::new().await;
    let binding = fixture.client.configure(&spec(), 0).await.unwrap().binding;
    fixture
        .client
        .resize_fenced(&binding, &target(1, 3))
        .await
        .unwrap();
    fixture.client.begin_retirement(&binding).await.unwrap();
    fixture.client.resize(&binding, 0).await.unwrap();
    assert_eq!(retained_target(&fixture, &binding), target(1, 3));
    assert!(fixture
        .client
        .resize_fenced(&binding, &target(1, 3))
        .await
        .is_err());
    assert!(fixture
        .client
        .resize_fenced(&binding, &target(3, 9))
        .await
        .is_err());
    fixture
        .client
        .resize_fenced(&binding, &target(2, 0))
        .await
        .unwrap();
    assert_eq!(retained_target(&fixture, &binding), target(2, 0));
    assert_eq!(
        fixture.row("sandboxwarmpools", &binding.name)["spec"]["replicas"],
        0
    );
}

#[tokio::test]
async fn invalid_target_is_rejected_before_provider_io() {
    let fixture = Fixture::new().await;
    let binding = fixture.client.configure(&spec(), 0).await.unwrap().binding;
    let before = fixture.state.lock().unwrap().calls.len();
    for invalid in [
        target(0, 2),
        target(1, 1001),
        NativePoolTarget {
            target_id: "unbounded-or-unknown-identity".into(),
            ..target(1, 1)
        },
    ] {
        assert!(fixture
            .client
            .resize_fenced(&binding, &invalid)
            .await
            .is_err());
    }
    assert_eq!(fixture.state.lock().unwrap().calls.len(), before);
}

use super::*;
use crate::sandbox::runtime_mapping::sandbox_spec;
use layer2_core::sandbox::{SandboxCustomRuntimeMigration, SandboxCustomRuntimeOrigin};

fn image(letter: char) -> String {
    format!(
        "registry.example/runtime@sha256:{}",
        letter.to_string().repeat(64)
    )
}

async fn active() -> (Fixture, Sandbox, SystemTime) {
    let mut fixture = Fixture::native().await;
    // Seed a fresh custom-profile Sandbox before any native authority exists.
    let mut created = fixture.harness.create().await;
    created.id = SandboxId(format!("{}_custom", created.id));
    created.name = "Custom runtime fixture".into();
    created.profile.content.runtime_image = Some(image('a'));
    created.profile.digest =
        layer2_core::profiles::content_digest(&created.profile.content).unwrap();
    fixture
        .harness
        .store
        .create_sandbox(created.clone())
        .await
        .unwrap();
    let mut now = start_time(&created);
    let active = fixture
        .drive(&created.id, &mut now, SandboxStatus::Active, &image('c'))
        .await;
    fixture.harness.adapter = fixture.adapter(&image('c'));
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        fixture.harness.adapter.as_ref(),
        &active,
        "test",
    )
    .await
    .unwrap();
    assert_eq!(active.runtime.as_ref().unwrap().runtime_version, image('a'));
    (fixture, active, now)
}

fn request(sandbox: &Sandbox, target: char) -> SandboxCustomRuntimeMigration {
    SandboxCustomRuntimeMigration {
        request_key: format!("custom-runtime-{target}"),
        expected_revision: sandbox.revision,
        expected_image: sandbox.effective_custom_runtime_image().unwrap().into(),
        target_image: image(target),
        origin: SandboxCustomRuntimeOrigin::capture(sandbox).unwrap(),
    }
}

#[tokio::test]
async fn custom_runtime_queued_target_preserves_exact_source_adoption() {
    let (fixture, active, now) = active().await;
    let mut queued = active.clone();
    queued
        .request_custom_runtime_migration(request(&active, 'b'), &image('b'), rfc3339_at(now))
        .unwrap();
    assert_eq!(
        sandbox_spec(&queued, "test").unwrap().runtime_image,
        Some(image('a'))
    );
    assert_eq!(queued.profile, active.profile);
    let source = crate::sandbox::runtime_attachment::retained_runtime_request(
        &queued,
        active.runtime.as_ref().unwrap(),
        "test",
    )
    .unwrap();
    assert_eq!(source.expected_image, Some(image('a')));
    assert_eq!(source.spec.runtime_image, Some(image('a')));
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        fixture.harness.adapter.as_ref(),
        &queued,
        "test",
    )
    .await
    .unwrap();
    assert!(!fixture.backend.calls.snapshot().contains(&"upgrade".into()));
    // The proposal is still local: admission must use the dedicated store path.
    assert_eq!(fixture.harness.stored(&active.id).await, active);
}

#[tokio::test]
async fn custom_runtime_invalid_authority_fails_before_provider_inspection() {
    let (fixture, active, now) = active().await;
    let mut queued = active.clone();
    queued
        .request_custom_runtime_migration(request(&active, 'b'), &image('b'), rfc3339_at(now))
        .unwrap();
    queued
        .lifecycle
        .custom_runtime
        .as_mut()
        .unwrap()
        .effective_image = image('d');
    let before = fixture.backend.calls.snapshot();
    let adoptions = fixture.backend.adoptions.load(Ordering::SeqCst);
    assert!(
        adoptions > 0,
        "the valid fixture must exercise provider adoption"
    );
    assert!(sandbox_spec(&queued, "test").is_err());
    assert!(crate::sandbox::runtime_attachment::adopt_bound_runtime(
        fixture.harness.adapter.as_ref(),
        &queued,
        "test",
    )
    .await
    .is_err());
    assert_eq!(fixture.backend.calls.snapshot(), before);
    assert_eq!(fixture.backend.adoptions.load(Ordering::SeqCst), adoptions);
    assert_eq!(fixture.harness.stored(&active.id).await, active);
}

#[path = "custom_runtime_lifecycle_tests.rs"]
mod lifecycle;

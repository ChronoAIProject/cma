use super::*;
use crate::{
    auth::test_resources,
    startup::fixed_pool_startup,
    store::{runtime_routing::MongoRuntimeRouting, MongoSandboxStore},
    warm_pools::startup::WarmPoolStartup,
};
use layer1_public::rest::admin::warm_pools::WarmPoolError;
use layer2_core::sandbox::{RuntimeBinding, SandboxRuntimeUpdateService};
use layer3_adapter::{
    hosted::{HostedDeployment, RuntimeBackends, RuntimeRoutingStore},
    AdapterError, SandboxAdapter, SandboxSpec,
};
use mongodb::bson::{doc, serialize_to_document, DateTime, Document};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fixed_pin_rollback_serves_with_the_conflicting_lane_held() {
    let fixture = Fixture::new().await;
    let db = &fixture.database;
    let store = Arc::new(MongoNativeWarmPools::open(db, DEPLOYMENT).await.unwrap());
    let sandbox_store = Arc::new(
        MongoSandboxStore::open(db.client(), db.name())
            .await
            .unwrap()
            .with_managed_runtime_version(image(Lane::Agent, 'a')),
    );
    let holds = sandbox_store.default_pool_holds.clone();
    let routing =
        MongoRuntimeRouting::open_with_default_pool_holds(db, DEPLOYMENT, true, holds.clone())
            .await
            .unwrap();
    let adapter = Arc::new(CodexAdapter::with_backend(
        RuntimeBackends {
            agent_sandbox: Some(Arc::new(resize::PoolBackend(fixture.provider.clone()))),
            opensandbox: None,
            allocate_agent_sandbox: true,
            routing: routing.clone(),
            home_routes: None,
        },
        CodexSandboxSpec {
            deployment: HostedDeployment {
                image: deployment_image(image(Lane::Agent, 'a')),
                ..Default::default()
            },
            ..Default::default()
        },
    ));
    let pools = |agent, target, machine| {
        NativeWarmPools::new(
            store.clone(),
            adapter.clone(),
            image(Lane::Agent, agent),
            Some(home_policy()),
            capacity(),
            target,
            machine,
        )
    };
    let b = fixture.plan(Lane::Agent, 'b', capacity());
    let machine = fixture.plan(Lane::Machine, '1', capacity());
    let rows = db.collection::<Document>("native_warm_pool_targets");
    assert!(matches!(
        fixed_pool_startup(&pools('a', 0, None), None)
            .await
            .unwrap(),
        WarmPoolStartup::ZeroTarget
    ));
    let old = resize::only(store.list().await.unwrap());
    rows.update_one(
        doc! {"_id": &old.id},
        doc! {"$set": {"activated_at": DateTime::from_millis(1)}},
    )
    .await
    .unwrap();
    // Run the normal startup path on B and bind its provider pool before rolling back.
    fixture
        .provider
        .configure_planned(&b, TARGET)
        .await
        .unwrap();
    fixture.readiness(&b, TARGET);
    assert!(matches!(
        fixed_pool_startup(&pools('b', TARGET, None), None)
            .await
            .unwrap(),
        WarmPoolStartup::Ready
    ));
    let before = store
        .list()
        .await
        .unwrap()
        .iter()
        .map(|row| serialize_to_document(row).unwrap())
        .collect::<Vec<_>>();
    let effect_count = fixture.provider.inventory.lock().unwrap().effects.len();
    let mut retained = test_resources::sandbox("retained-on-b", "owner");
    retained.runtime = Some(RuntimeBinding {
        runtime_id: "agent-sandbox:retained:uid".into(),
        runtime_version: image(Lane::Agent, 'b'),
        provision_generation: Some(1),
        boot_id: None,
        bound_at: layer2_core::now_rfc3339(),
    });
    let old_pin = image(Lane::Agent, 'a');
    let updates = SandboxRuntimeUpdateService::new(sandbox_store.clone(), old_pin.clone());
    assert!(updates.target_is_qualified(&retained, &old_pin));
    let rollback =
        pools('a', 0, Some((image(Lane::Machine, '1'), TARGET))).with_default_pool_holds(holds);
    assert!(matches!(
        rollback.ensure_default().await,
        Err(WarmPoolError::Conflict)
    ));
    assert!(matches!(
        fixed_pool_startup(&rollback, None).await.unwrap(),
        WarmPoolStartup::ConfigurationDrift
    ));
    assert!(rollback.default_held(false));
    assert_eq!(sandbox_store.managed_agent_release(), None);
    assert!(!updates.target_is_qualified(&retained, &old_pin));

    let fresh = SandboxSpec {
        sandbox_id: "rollback-fresh".into(),
        ..Default::default()
    };
    assert!(
        matches!(adapter.provision(&fresh).await, Err(AdapterError::Unavailable(reason)) if reason.contains("conflicts with durable pool authority"))
    );
    assert!(routing
        .require_allocation_image(&image(Lane::Machine, '1'))
        .is_ok());
    assert!(rollback.reconcile_one().await.unwrap());
    assert!(!rollback.reconcile_one().await.unwrap());
    let after = store.list().await.unwrap();
    assert_eq!(
        after
            .iter()
            .filter(|row| !row.machine)
            .map(|row| serialize_to_document(row).unwrap())
            .collect::<Vec<_>>(),
        before
    );
    {
        let inventory = fixture.provider.inventory.lock().unwrap();
        assert_eq!(inventory.status(&b).desired_replicas, TARGET);
        assert_eq!(inventory.status(&machine).desired_replicas, TARGET);
        let machine_effects = &inventory.effects[effect_count..];
        assert!(machine_effects.iter().all(|effect| matches!(effect,
            Effect::Configure(hash, _) | Effect::Resize(hash, _) if hash == &machine.config_sha256)));
    }
    db.drop().await.unwrap();
}

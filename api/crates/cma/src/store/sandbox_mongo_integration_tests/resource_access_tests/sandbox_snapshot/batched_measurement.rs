use super::{batch_fixture::*, *};
use layer2_core::sandbox::{
    colors::{SandboxColorService, SystemColorRandom},
    SandboxDetailPhase,
};
use layer2_core::{PublicIdentity, UserDirectory, UserDirectoryStore};
use std::time::Instant;

const SAMPLES: usize = 20;
async fn measure(label: &str, workspaces: u32, per_workspace: u32) {
    let (admin, name) = database("sandbox_batch_measure").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let template = template(&store, &target).await;
    let root = seed(&store, &template, label, workspaces, per_workspace).await;
    // Unrelated retained populations share the collections but must never join.
    for i in 0..4 {
        seed(&store, &template, &format!("neighbor-{i}"), 16, 16).await;
    }
    let colors_store = Arc::new(
        crate::store::sandbox_colors::MongoSandboxColorStore::open(&client.database(&name))
            .await
            .unwrap(),
    );
    let names_store = Arc::new(
        crate::store::directory::MongoUserDirectoryStore::open(&client, &name)
            .await
            .unwrap(),
    );
    names_store
        .observe(&PublicIdentity {
            subject: root.owner.clone(),
            display_name: Some("Fixture".into()),
            avatar_url: None,
        })
        .await
        .unwrap();
    let names = UserDirectory::new(names_store);
    let caller = Caller {
        subject: root.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    let services: Vec<_> = (0..3).map(|m| service(mode(&store, m))).collect();
    let colors = SandboxColorService::new(
        colors_store,
        Arc::new(store.clone()),
        Arc::new(SystemColorRandom::new()),
    );
    let run = |m: usize| {
        let services = &services;
        let caller = &caller;
        let root = &root;
        let colors = &colors;
        let names = &names;
        async move {
            let view = services[m].sandbox(caller, &root.id).await.unwrap();
            let timer = services[m].detail_phase(SandboxDetailPhase::Colors);
            assert_eq!(
                colors.colors(&caller.subject, &[&view.sandbox]).await.len(),
                1
            );
            timer.complete();
            assert_eq!(
                services[m]
                    .detail_phase(SandboxDetailPhase::Names)
                    .result(names.display_names(std::slice::from_ref(&caller.subject)))
                    .await
                    .unwrap()
                    .len(),
                1
            );
            view
        }
    };
    let expected = run(0).await;
    for m in 0..3 {
        assert_eq!(run(m).await, expected);
    }
    assert_eq!(expected.counts.workspaces, u64::from(workspaces));
    assert_eq!(
        expected.counts.worktrees,
        u64::from(workspaces * per_workspace)
    );
    assert_eq!(
        expected.counts.agents,
        u64::from(workspaces * per_workspace)
    );
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::INFO)
            .finish(),
    );
    let mut times: [Vec<u128>; 3] = Default::default();
    let mut checkouts = [0_u64; 3];
    let mut get_more = [0_usize; 3];
    for sample in 0..SAMPLES {
        // Rotate the order to avoid a fixed warm-cache / neighbor-load bias.
        for offset in 0..3 {
            let m = (sample + offset) % 3;
            clear(&trace);
            println!("BATCH_SAMPLE volume={label} mode={m} sample={sample}");
            let start = Instant::now();
            assert_eq!(run(m).await, expected);
            times[m].push(start.elapsed().as_micros());
            checkouts[m] += trace.checkouts.load(Ordering::SeqCst);
            get_more[m] += trace
                .commands
                .lock()
                .unwrap()
                .iter()
                .filter(|(n, _)| n == "getMore")
                .count();
        }
    }
    for m in 0..3 {
        times[m].sort_unstable();
        println!("BATCH_MEASURE volume={label} mode={m} workspaces={workspaces} worktrees={} agents={} unrelated_agents=1025 samples={SAMPLES} p50_us={} p95_us={} max_us={} checkouts={} get_more={}",
            workspaces * per_workspace, workspaces * per_workspace, times[m][9],times[m][18],times[m][19],checkouts[m],get_more[m]);
    }
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn batched_measurement_small_hierarchy() {
    measure("small", 4, 4).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn batched_measurement_medium_hierarchy() {
    measure("medium", 16, 16).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn batched_measurement_large_hierarchy() {
    measure("large", 128, 32).await;
}

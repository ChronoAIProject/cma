use super::{batch_fixture::*, *};
use layer2_core::sandbox::{
    colors::{SandboxColorService, SystemColorRandom},
    SandboxDetailPhase,
};
use layer2_core::{PublicIdentity, UserDirectory, UserDirectoryStore};
use std::time::Instant;

#[path = "access_measurement_trace.rs"]
mod trace;
const SAMPLES: usize = 20;
const MODES: usize = 6;
// Baseline, access only, enrichment only, both, existing hierarchy flags, all.
const SETTINGS: [(usize, bool, bool); MODES] = [
    (0, false, false),
    (0, true, false),
    (0, false, true),
    (0, true, true),
    (2, false, false),
    (2, true, true),
];
async fn measure(label: &str, workspaces: u32, per_workspace: u32) {
    let (admin, name) = database("sandbox_batch_measure").await;
    let trace = Arc::new(trace::Meter::default());
    let client = trace::client(&trace).await;
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
    let services: Vec<_> = SETTINGS
        .iter()
        .map(|&(hierarchy, access, _)| {
            service(mode(&store, hierarchy).with_access_snapshot_session(access))
        })
        .collect();
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
            let color = async {
                let timer = services[m].detail_phase(SandboxDetailPhase::Colors);
                let values = timer
                    .scope(colors.colors(&caller.subject, &[&view.sandbox]))
                    .await;
                timer.complete();
                assert_eq!(values.len(), 1);
            };
            let name = async {
                assert_eq!(
                    services[m]
                        .detail_phase(SandboxDetailPhase::Names)
                        .result(names.display_names(std::slice::from_ref(&caller.subject)))
                        .await
                        .unwrap()
                        .len(),
                    1
                );
            };
            if SETTINGS[m].2 {
                tokio::join!(color, name);
            } else {
                color.await;
                name.await;
            }
            view
        }
    };
    let expected = run(0).await;
    for m in 0..MODES {
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
    let mut times: [Vec<u128>; MODES] = Default::default();
    let mut checkouts = [0_u64; MODES];
    let mut get_more = [0_usize; MODES];
    for sample in 0..SAMPLES {
        // Rotate the order to avoid a fixed warm-cache / neighbor-load bias.
        for offset in 0..MODES {
            let m = (sample + offset) % MODES;
            trace.clear();
            println!("ACCESS_SAMPLE volume={label} mode={m} sample={sample}");
            let start = Instant::now();
            assert_eq!(run(m).await, expected);
            times[m].push(start.elapsed().as_micros());
            checkouts[m] += trace.checkouts.load(Ordering::SeqCst);
            let stats = trace.stats.lock().unwrap().clone();
            let base = match workspaces {
                4 => 13,
                16 => 15,
                128 => 16,
                _ => unreachable!(),
            };
            let expected = match m {
                0 | 2 => base,
                1 | 3 => base - 2,
                4 => {
                    if workspaces == 4 {
                        10
                    } else {
                        11
                    }
                }
                5 => {
                    if workspaces == 4 {
                        8
                    } else {
                        9
                    }
                }
                _ => unreachable!(),
            };
            assert_eq!(stats.count(), expected);
            assert_eq!(trace.checkouts.load(Ordering::SeqCst), expected);
            assert_eq!(stats.agent_docs, u64::from(workspaces * per_workspace));
            assert!(stats.commands.keys().all(|key| !key.contains("agent_log")));
            get_more[m] += stats
                .commands
                .iter()
                .filter(|(key, _)| key.ends_with("/getMore"))
                .map(|(_, count)| *count as usize)
                .sum::<usize>();
            if sample == 0 {
                println!("ACCESS_COMMANDS volume={label} mode={m} commands={:?} returned_docs={} returned_bson_bytes={} hierarchy_agent_docs={} hierarchy_agent_bson_bytes={} agent_log_scans=0",
                    stats.commands, stats.docs, stats.bytes, stats.agent_docs, stats.agent_bytes);
            }
        }
    }
    for m in 0..MODES {
        times[m].sort_unstable();
        println!("ACCESS_MEASURE volume={label} mode={m} workspaces={workspaces} worktrees={} agents={} unrelated_agents=1025 samples={SAMPLES} p50_us={} p95_us={} max_us={} checkouts={} get_more={}",
            workspaces * per_workspace, workspaces * per_workspace, times[m][9],times[m][18],times[m][19],checkouts[m],get_more[m]);
    }
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn access_measurement_small_hierarchy() {
    measure("small", 4, 4).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn access_measurement_medium_hierarchy() {
    measure("medium", 16, 16).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn access_measurement_large_hierarchy() {
    measure("large", 128, 32).await;
}

use super::*;
use crate::agent_sandbox::pool::read_fixture::Effects;

fn policy_paths(fixture: &Resume) -> [String; 2] {
    let pool = &fixture
        .home
        .receipt
        .native_authority
        .as_ref()
        .unwrap()
        .claim
        .request
        .pool;
    [
        format!("{}/sandboxtemplates/{}", prefix(), pool.template_name),
        format!("{}/sandboxwarmpools/{}", prefix(), pool.name),
    ]
}

#[tokio::test]
async fn native_resume_drops_delayed_retired_policy_only_for_proven_independent_upgrades() {
    for (upgraded, network) in [(false, false), (true, false), (true, true)] {
        let fixture = fixture(network).await;
        let runtime = client(&fixture.fixture, true);
        let (previous, next) = if upgraded {
            upgrade(&fixture, &fixture.previous, &fixture.next)
                .await
                .unwrap();
            runtime
                .suspend(&fixture.handle, &fixture.next)
                .await
                .unwrap();
            (fixture.next.clone(), target(&fixture.home, 3, &image('b')))
        } else {
            (fixture.previous.clone(), request(&fixture.home, 2))
        };
        let exempt = upgraded && !network;
        if exempt {
            remove_definitions(&fixture);
        }
        let claim_gate = Gate::new();
        let policy_gates: BTreeMap<_, _> = policy_paths(&fixture)
            .into_iter()
            .map(|path| (path, Gate::new()))
            .collect();
        let mut gates = policy_gates.clone();
        gates.insert(fixture.claim_path.clone(), claim_gate.clone());
        let effects = Effects::new(&fixture.fixture, gates);
        let runtime = AgentSandboxClient::new(
            effects.client.clone(),
            fixture.fixture.client.config.clone(),
            "fixture",
        )
        .unwrap();
        let (handle, home) = (fixture.handle.clone(), fixture.home.clone());
        let mut task = tokio::spawn(async move {
            runtime
                .resume_with_home(&handle, &previous, &next, &home)
                .await
        });
        claim_gate.wait().await;
        for gate in policy_gates.values() {
            gate.wait().await;
        }
        claim_gate.respond();
        if !exempt {
            assert!(tokio::time::timeout(Duration::from_millis(25), &mut task)
                .await
                .is_err());
            effects.assert_read_only();
            for gate in policy_gates.values() {
                gate.respond();
            }
        }
        // An exempt home completes its actual resume while both old-definition
        // responses remain blocked. Other homes require those responses first.
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            effects
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| call.starts_with("PATCH "))
                .count(),
            1
        );
        let state = fixture.fixture.state.lock().unwrap();
        assert_eq!(state.rows[MEMBER]["spec"]["operatingMode"], "Running");
        assert_eq!(state.rows[VOLUME]["metadata"]["uid"], "pvc-uid");
    }
}

#[tokio::test]
async fn native_resume_returns_graph_failure_without_waiting_for_stalled_policy() {
    let fixture = fixture(false).await;
    let claim_gate = Gate::new();
    let policy_gates: BTreeMap<_, _> = policy_paths(&fixture)
        .into_iter()
        .map(|path| (path, Gate::new()))
        .collect();
    let mut gates = policy_gates.clone();
    gates.insert(fixture.claim_path.clone(), claim_gate.clone());
    let (reads, runtime) = fixture.reads(
        BTreeMap::from([(fixture.claim_path.clone(), failure(429))]),
        gates,
    );
    let next = request(&fixture.home, 2);
    let task = tokio::spawn(async move {
        runtime
            .resume_with_home(&fixture.handle, &fixture.previous, &next, &fixture.home)
            .await
    });
    claim_gate.wait().await;
    for gate in policy_gates.values() {
        gate.wait().await;
    }
    claim_gate.respond();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap(),
        Err(crate::AdapterError::AtCapacity(_))
    ));
    assert_eq!(reads.calls.lock().unwrap().len(), 5);
}

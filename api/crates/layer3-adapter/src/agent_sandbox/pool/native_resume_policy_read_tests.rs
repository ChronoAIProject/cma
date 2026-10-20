use super::*;

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
async fn native_resume_policy_and_graph_reads_overlap_and_replay_reacquires_both() {
    let fixture = Resume::with_policy(true).await;
    fixture.resume().await.unwrap();
    for _ in 0..2 {
        let paths = [fixture.claim_path.clone(), MEMBER.into(), VOLUME.into()]
            .into_iter()
            .chain(policy_paths(&fixture));
        let gates: BTreeMap<_, _> = paths.map(|path| (path, Gate::new())).collect();
        let (reads, client) = fixture.reads(BTreeMap::new(), gates.clone());
        let (handle, previous, next, home) = (
            fixture.handle.clone(),
            fixture.previous.clone(),
            fixture.next.clone(),
            fixture.home.clone(),
        );
        let task = tokio::spawn(async move {
            client
                .resume_with_home(&handle, &previous, &next, &home)
                .await
        });
        // Neither wave may need a response from the other to start.
        for gate in gates.values() {
            gate.wait().await;
        }
        assert_eq!(reads.calls.lock().unwrap().len(), 5);
        assert!(!task.is_finished());
        for gate in gates.values() {
            gate.respond();
        }
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            reads.calls.lock().unwrap().len(),
            6,
            "fresh graph and policy plus the idempotency check"
        );
    }
}

#[tokio::test]
async fn native_resume_graph_error_precedes_faster_policy_failure_without_writes() {
    let fixture = Resume::with_policy(true).await;
    let [template, _] = policy_paths(&fixture);
    let claim_gate = Gate::new();
    let (reads, client) = fixture.reads(
        BTreeMap::from([
            (fixture.claim_path.clone(), failure(429)),
            (template, failure(403)),
        ]),
        BTreeMap::from([(fixture.claim_path.clone(), claim_gate.clone())]),
    );
    let mut task = tokio::spawn(async move {
        client
            .resume_with_home(
                &fixture.handle,
                &fixture.previous,
                &fixture.next,
                &fixture.home,
            )
            .await
    });
    claim_gate.wait().await;
    assert!(tokio::time::timeout(Duration::from_millis(25), &mut task)
        .await
        .is_err());
    assert_eq!(reads.calls.lock().unwrap().len(), 5);
    claim_gate.respond();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap(),
        Err(crate::AdapterError::AtCapacity(_))
    ));
}

#[tokio::test]
async fn native_resume_rejects_changed_or_unavailable_policy_before_transition() {
    let fixture = Resume::with_policy(true).await;
    let [template_path, pool_path] = policy_paths(&fixture);
    for (path, change) in [
        (&template_path, "api"),
        (&template_path, "uid"),
        (&template_path, "spec"),
        (&pool_path, "uid"),
    ] {
        let mut value = fixture.fixture.state.lock().unwrap().rows[path].clone();
        match change {
            "uid" => value["metadata"]["uid"] = json!("replacement"),
            "spec" => {
                value["spec"]["podTemplate"]["spec"]["securityContext"]["fsGroupChangePolicy"] =
                    json!("Always");
            }
            _ => (),
        }
        let response = if change == "api" {
            failure(429)
        } else {
            (200, value)
        };
        let (reads, client) =
            fixture.reads(BTreeMap::from([(path.clone(), response)]), BTreeMap::new());
        let result = client
            .resume_with_home(
                &fixture.handle,
                &fixture.previous,
                &fixture.next,
                &fixture.home,
            )
            .await;
        assert!(result.is_err(), "{path}: {change}");
        assert_eq!(reads.calls.lock().unwrap().len(), 5);
    }
}

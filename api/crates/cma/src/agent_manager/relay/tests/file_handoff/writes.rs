use super::*;
use base64::Engine as _;

fn edit(peer: &Replica, id: &AgentId, content: &[u8]) -> reqwest::RequestBuilder {
    client()
        .post(format!(
            "{}/api/v1/agents/{id}/files",
            peer.instance.address
        ))
        .json(&serde_json::json!({
            "path": FILE_PATH,
            "expected_sha256": runtime::digest(),
            "content_base64": base64::engine::general_purpose::STANDARD.encode(content),
        }))
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn only_a_proved_pre_entry_refusal_recovers_a_concurrent_file_edit() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_file_edit_{}", uuid::Uuid::new_v4().simple());
    let db = client.database(&name);
    let agent = resources(&client, &name).await;
    let contents = Arc::new(Mutex::new(FILE_BYTES.to_vec()));
    let first = Replica::bind(&uri, &db, &agent, 0, Some(contents.clone())).await;
    let second = Replica::bind(&uri, &db, &agent, 1, Some(contents.clone())).await;
    let membership = first.instance.coordinator.store.membership().await.unwrap();
    let preferred = &candidates(&agent.id, &membership, None).unwrap()[0].instance_id;
    let (owner, ingress) = if first.instance.coordinator.instance_id == *preferred {
        (&first, &second)
    } else {
        (&second, &first)
    };
    let owned = owner
        .instance
        .coordinator
        .acquire(&agent.id)
        .await
        .unwrap()
        .unwrap();

    owner.runtime.lose_write_ack.store(true, SeqCst);
    let uncertain = edit(ingress, &agent.id, b"committed before reply")
        .send()
        .await
        .unwrap();
    assert_eq!(uncertain.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        uncertain.json::<Value>().await.unwrap()["code"],
        "file_unavailable"
    );
    assert_eq!(owner.runtime.writes.load(SeqCst), 1);
    assert_eq!(ingress.runtime.writes.load(SeqCst), 0);
    assert_eq!(*contents.lock().unwrap(), b"committed before reply");
    *contents.lock().unwrap() = FILE_BYTES.to_vec();

    owner.instance.coordinator.stop_admission();
    assert_eq!(
        ingress
            .instance
            .relay
            .destination(&agent.id)
            .await
            .unwrap()
            .token,
        owned.token
    );
    let response = edit(ingress, &agent.id, b"recovered edit")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    ingress.relay_log.assert_correlated(
        &owner.relay_log,
        "write",
        "verified_write_not_entered",
        StatusCode::OK,
    );
    assert!(!response
        .headers()
        .contains_key(file_write::NOT_ENTERED_HEADER));
    assert_eq!(owner.runtime.writes.load(SeqCst), 1);
    assert_eq!(ingress.runtime.writes.load(SeqCst), 1);
    assert_eq!(*contents.lock().unwrap(), b"recovered edit");
    *contents.lock().unwrap() = FILE_BYTES.to_vec();

    let variants = [
        b"first candidate".as_slice(),
        b"second candidate".as_slice(),
    ];
    let (left, right) = tokio::join!(
        edit(ingress, &agent.id, variants[0]).send(),
        edit(ingress, &agent.id, variants[1]).send(),
    );
    let statuses = [left.unwrap().status(), right.unwrap().status()];
    let winner = statuses
        .iter()
        .position(|status| *status == StatusCode::OK)
        .unwrap();
    assert_eq!(statuses[1 - winner], StatusCode::CONFLICT);
    assert_eq!(owner.runtime.writes.load(SeqCst), 1);
    assert_eq!(ingress.runtime.writes.load(SeqCst), 3);
    let retrieved = super::client()
        .get(format!(
            "{}/api/v1/agents/{}/files/download",
            ingress.instance.address, agent.id
        ))
        .query(&[("path", FILE_PATH)])
        .send()
        .await
        .unwrap();
    assert_eq!(retrieved.status(), StatusCode::OK);
    assert_eq!(retrieved.bytes().await.unwrap().as_ref(), variants[winner]);

    owner.probe.pause_write_refusal.store(true, SeqCst);
    let request = edit(ingress, &agent.id, b"must not enter replacement runtime").send();
    let replace = async {
        tokio::time::timeout(Duration::from_secs(2), owner.probe.write_refused.notified())
            .await
            .unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let mut sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
        let binding = sandbox.runtime.as_mut().unwrap();
        binding.runtime_id = "replacement-file-edit-runtime".into();
        binding.boot_id = Some("b".repeat(32));
        sandbox.revision += 1;
        store
            .save_sandbox(sandbox.revision - 1, sandbox)
            .await
            .unwrap();
        owner.probe.resume_write_reply.notify_one();
    };
    let (response, ()) = tokio::join!(request, replace);
    assert_eq!(response.unwrap().status(), StatusCode::CONFLICT);
    ingress.relay_log.assert_correlated(
        &owner.relay_log,
        "write",
        "verified_write_not_entered",
        StatusCode::CONFLICT,
    );
    assert_eq!(owner.runtime.writes.load(SeqCst), 1);
    assert_eq!(ingress.runtime.writes.load(SeqCst), 3);
    db.drop().await.unwrap();
}

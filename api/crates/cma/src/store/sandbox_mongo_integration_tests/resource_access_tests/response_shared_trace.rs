use super::*;
use sha2::{Digest, Sha256};
use std::{
    io::{self, Write},
    sync::{atomic::Ordering, Mutex},
};

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl Write for Output {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;

    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

impl Output {
    fn events(&self) -> Vec<Value> {
        let bytes = self.0.lock().unwrap();
        std::str::from_utf8(&bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()["fields"].clone())
            .collect()
    }
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn response_open_trace_joins_concurrent_attempts_without_changing_wire_results() {
    let output = Output::default();
    let dispatch = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_env_filter("cma::agent_manager::observation_open=debug")
            .with_writer(output.clone())
            .json()
            .finish(),
    );
    let (f, id) = retained(100).await;
    let replicas = traced_servers(&f, dispatch).await;
    let client = client();
    let path = format!("/api/v2/responses/{id}/events");
    let destination = replicas[0]
        .observation
        .destination(&f.agent.id, &f.agent.created_at)
        .await
        .unwrap();
    let owner = replicas
        .iter()
        .position(|replica| replica.address == destination.member.relay_address)
        .unwrap();
    let peer = 1 - owner;
    let expected_epoch = destination.authority.token.epoch;
    let expected_owner = &destination.authority.token.instance_id;
    let open = |index: usize, attempt: uuid::Uuid| {
        request(&client, &f, &replicas[index], &path)
            .header("x-request-id", attempt.to_string())
            .send()
    };

    let default_off = open(peer, uuid::Uuid::new_v4()).await.unwrap();
    assert_eq!(default_off.status(), reqwest::StatusCode::OK);
    drop(default_off);
    assert!(output.events().is_empty());

    for replica in &replicas {
        replica.relay.enable_observation_open_trace();
    }
    let success_id = uuid::Uuid::new_v4();
    let success = open(peer, success_id).await.unwrap();
    assert_eq!(success.status(), reqwest::StatusCode::OK);
    assert_eq!(success.headers()["content-type"], "text/event-stream");
    drop(success);

    replicas[owner].stall.store(true, Ordering::SeqCst);
    let concurrent = [
        uuid::Uuid::new_v4(),
        uuid::Uuid::parse_str("00112233-4455-1677-8899-aabbccddeeff").unwrap(),
    ];
    let (left, right) = tokio::join!(open(peer, concurrent[0]), open(peer, concurrent[1]));
    for response in [left.unwrap(), right.unwrap()] {
        assert_unavailable(response).await;
    }
    replicas[owner].stall.store(false, Ordering::SeqCst);
    replicas[peer].coordinator.begin_drain().await;
    let unknown_id = uuid::Uuid::new_v4();
    assert_unavailable(open(peer, unknown_id).await.unwrap()).await;
    let snapshot = request(
        &client,
        &f,
        &replicas[peer],
        &format!("/api/v2/responses/{id}"),
    )
    .header("x-request-id", uuid::Uuid::new_v4().to_string())
    .send()
    .await
    .unwrap();
    assert_unavailable(snapshot).await;

    let recorded = output.events();
    assert_eq!(recorded.len(), 4);
    let events_for = |id: uuid::Uuid| {
        let hash = format!("{:x}", Sha256::digest(id.to_string()));
        recorded
            .iter()
            .filter(|row| row["request_sha256"] == hash)
            .collect::<Vec<_>>()
    };
    let success = events_for(success_id);
    assert_eq!(success.len(), 1);
    assert_eq!(success[0]["branch"], "sse_opened");
    assert_eq!(success[0]["hop"], "ingress");
    assert_eq!(success[0]["read_epoch"].as_u64(), Some(expected_epoch));

    for id in concurrent {
        let rows = events_for(id);
        assert_eq!(rows.len(), 1);
        let row = rows[0];
        assert_eq!(row["branch"], "peer_admission_deadline");
        assert_eq!(row["hop"], "ingress");
        assert_eq!(row["http_status"], 503);
        assert_eq!(row["read_epoch"].as_u64(), Some(expected_epoch));
        assert_eq!(row["read_owner"], expected_owner.as_str());
        assert!(row["source_file"]
            .as_str()
            .unwrap()
            .ends_with("/observation.rs"));
        assert!(row["source_line"].as_u64().unwrap() > 0);
    }

    let rows = events_for(unknown_id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["branch"], "coordinator_not_ready");
    assert!(rows[0]["read_epoch"].is_null());
    assert!(rows[0]["read_owner"].is_null());
    assert!(rows[0]["signed_epoch"].is_null());
    assert!(!String::from_utf8(output.0.lock().unwrap().clone())
        .unwrap()
        .contains(id.as_str()));

    drop(replicas);
    f.close().await;
}

async fn assert_unavailable(response: reqwest::Response) {
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    assert_eq!(
        response.json::<Value>().await.unwrap()["code"],
        "agent_owner_unavailable"
    );
}

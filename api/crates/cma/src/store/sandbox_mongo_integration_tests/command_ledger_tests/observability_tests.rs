use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admission_metrics_distinguish_new_replay_and_refusal_without_new_effects() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _metrics = metrics::set_default_local_recorder(&recorder);
    let (client, name) = database("admission_observability").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let original = request(&agent, &parent, "observability-contract");
    let accepted = record(store.reserve_command(original.clone()).await.unwrap());
    let replay = record(store.reserve_command(original.clone()).await.unwrap());
    assert_eq!(accepted, replay);
    let mut changed = original;
    changed.envelope.command = AgentCommand::message("A different command");
    changed.envelope.request_fingerprint = changed.envelope.command.fingerprint().unwrap();
    assert_eq!(
        store.reserve_command(changed).await.unwrap_err(),
        CommandLedgerError::IdempotencyConflict
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    let text = handle.render();
    for outcome in ["created", "existing", "agent_command_key_conflict"] {
        assert!(
            text.lines()
                .any(|line| line.starts_with("cma_admission_attempts_total{")
                    && line.contains(&format!("outcome=\"{outcome}\""))
                    && line.ends_with(" 1")),
            "missing {outcome}: {text}"
        );
    }
    for private in [
        "observability-contract",
        agent.id.as_str(),
        "different command",
    ] {
        assert!(!text.contains(private));
    }
    client.database(&name).drop().await.unwrap();
}

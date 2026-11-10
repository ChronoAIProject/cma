use super::bounded_fixtures::prepare_history;
use super::*;
use crate::store::agent_log_integrity::parse_integrity;
use crate::store::agent_log_telemetry::{snapshot, ScanCaller};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn single_append_unknown_commit_uses_one_snapshot_and_never_inserts_twice() {
    for enabled in [false, true] {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let app_name = format!("bounded-receipt-{}", uuid::Uuid::new_v4().simple());
        let name = format!("cma_bounded_receipt_{}", uuid::Uuid::new_v4().simple());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let observed = trace.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app_name.clone());
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                observed
                    .lock()
                    .unwrap()
                    .push((event.command_name, event.command));
            }
        }));
        let client = Client::with_options(options).unwrap();
        let (legacy, agent, _) = fixture(&name, &client).await;
        let (mut store, agent) = prepare_history(legacy, agent, 1000).await;
        if !enabled {
            store.bounded_admission_agents.clear();
        }
        trace.lock().unwrap().clear();
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": { "times": 5_i32 },
                "data": { "failCommands": ["commitTransaction"], "appName": app_name,
                    "writeConcernError": { "code": 50_i32, "codeName": "MaxTimeMSExpired",
                        "errmsg": "injected bounded append commit uncertainty" } }
            })
            .await
            .unwrap();
        // Reconciliation runs in a spawned task, outside isolated task-local
        // counters. Nextest isolates this test process; observe the real shared
        // production counters across that spawn boundary.
        let scans_before = snapshot(ScanCaller::UnknownCommitProof, true)[0];
        let result = store
            .append_agent_log(
                &agent.id,
                NOW.into(),
                AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::None,
                },
            )
            .await;
        let scans = snapshot(ScanCaller::UnknownCommitProof, true)[0] - scans_before;
        admin
            .database("admin")
            .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
            .await
            .unwrap();
        let appended = result.unwrap();
        assert_eq!(appended.seq, 1001);
        assert_eq!(scans, u64::from(!enabled));
        let commands = trace.lock().unwrap().clone();
        let commits = commands
            .iter()
            .filter(|(name, _)| name == "commitTransaction")
            .map(|(_, command)| command)
            .collect::<Vec<_>>();
        assert_eq!(commits.len(), 5);
        assert!(commits
            .iter()
            .all(|command| command.get("lsid") == commits[0].get("lsid")
                && command.get("txnNumber") == commits[0].get("txnNumber")));
        assert_eq!(
            commands
                .iter()
                .filter(|(name, command)| name == "insert"
                    && command.get_str("insert").ok() == Some("agent_log"))
                .count(),
            1
        );
        let last_commit = commands
            .iter()
            .rposition(|(name, _)| name == "commitTransaction")
            .unwrap();
        let reads = commands
            .iter()
            .skip(last_commit + 1)
            .filter(|(name, _)| name == "find")
            .map(|(_, command)| command)
            .collect::<Vec<_>>();
        assert!(!reads.is_empty());
        let concern = reads[0].get_document("readConcern").unwrap();
        assert_eq!(concern.get_str("level").unwrap(), "snapshot");
        let at = concern.get_timestamp("atClusterTime").unwrap();
        assert!(reads
            .iter()
            .all(|command| command.get("lsid") == reads[0].get("lsid")
                && command
                    .get_document("readConcern")
                    .is_ok_and(|concern| concern.get_str("level").ok() == Some("snapshot")
                        && concern.get_timestamp("atClusterTime").ok() == Some(at))));
        assert_eq!(
            store
                .agent_log_entry(&agent.id, appended.seq)
                .await
                .unwrap(),
            Some(appended)
        );
        assert_eq!(
            store
                .append_agent_log(
                    &agent.id,
                    NOW.into(),
                    AgentLogKind::WorkStepCompleted {
                        step: AgentWorkStep::None,
                    }
                )
                .await
                .unwrap()
                .seq,
            1002
        );
        assert_eq!(
            store
                .agent_log
                .count_documents(doc! { "agent_id": agent.id.as_str() })
                .await
                .unwrap(),
            1002
        );
        let row = store
            .agents
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(parse_integrity(&row).unwrap().unwrap().entry_count, 1002);
        client.database(&name).drop().await.unwrap();
    }
}

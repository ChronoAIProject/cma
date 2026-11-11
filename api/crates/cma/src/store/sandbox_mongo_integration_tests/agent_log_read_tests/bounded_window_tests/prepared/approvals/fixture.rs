use super::*;
use crate::store::agent_log_integrity::scan_complete_log_with_snapshot_session;
use crate::store::agent_log_telemetry::ScanCaller;

impl Fixture {
    pub(super) async fn approvals(mut self, long: bool, oversized: bool) -> Self {
        let mut agent = self.store.get_agent(&self.id).await.unwrap().unwrap();
        let head = agent.conversation.last_seq;
        let turn = if long { 3 } else { head - 9 };
        // The long-turn fallback must retain an unresolved request outside the
        // suffix. A later reset must clear it in the short-turn case.
        self.replace(
            100,
            AgentLogKind::ApprovalRequested {
                approval_id: "earlier".into(),
                action: Some("Prior request".into()),
                detail: None,
                runtime_locator: Some("earlier-route".into()),
            },
        )
        .await;

        if !long {
            self.replace(head - 10, AgentLogKind::TurnCompleted { turn_sequence: 3 })
                .await;
            self.replace(
                turn,
                AgentLogKind::TurnStarted {
                    turn_sequence: turn,
                    origin_key: None,
                    model: None,
                },
            )
            .await;
            self.replace(
                turn + 1,
                AgentLogKind::OwnerMessage {
                    content: "approval task".into(),
                },
            )
            .await;
        }
        if oversized {
            self.replace(
                head - 6,
                AgentLogKind::AgentMessage {
                    content: "x".repeat(4 * 1024 * 1024),
                },
            )
            .await;
        }
        for (seq, id) in [
            (head - 4, "z"),
            (head - 3, "a"),
            (head - 1, "z"),
            (head, "b"),
        ] {
            self.replace(
                seq,
                AgentLogKind::ApprovalRequested {
                    approval_id: id.into(),
                    action: Some("Check".into()),
                    detail: Some(format!("request-{seq}")),
                    runtime_locator: Some(format!("route-{seq}")),
                },
            )
            .await;
        }
        self.replace(
            head - 2,
            AgentLogKind::ApprovalResolved {
                approval_id: "z".into(),
                decision: "approved".into(),
            },
        )
        .await;
        agent.conversation.active_turn = Some(turn);
        agent.status = AgentStatus::WaitingApproval;
        agent.revision += 1;
        let mut fields = agent_replacement_fields(&agent).unwrap();
        fields.insert("last_seq", head as i64);
        self.change_proof(doc! { "$set": fields, "$unset": { INTEGRITY_FIELD: "" }})
            .await;
        self.store = self
            .store
            .with_bounded_approval_reads(true)
            .prepare_bounded_admission(std::slice::from_ref(&self.id))
            .await
            .unwrap();
        self
    }

    pub(super) async fn replace(&self, seq: u64, kind: AgentLogKind) {
        self.store
            .agent_log
            .replace_one(
                doc! { "agent_id": self.id.as_str(), "seq": seq as i64 },
                log_document(&AgentLogEntry {
                    agent_id: self.id.clone(),
                    seq,
                    appended_at: NOW.into(),
                    kind,
                })
                .unwrap(),
            )
            .await
            .unwrap();
    }

    pub(super) async fn change_proof(&self, update: Document) {
        self.store
            .agents
            .update_one(doc! { "_id": self.id.as_str() }, update)
            .await
            .unwrap();
    }

    // Exact pre-change reader, including the duplicate envelope read and shared
    // proof/audit wrapper. Used only on valid fixtures as a measured comparator.
    pub(super) async fn original_approvals(&self) -> Vec<AgentLogEntry> {
        let mut session = self.client.start_session().snapshot(true).await.unwrap();
        let document = self
            .store
            .agents
            .find_one(doc! { "_id": self.id.as_str() })
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .unwrap()
            .unwrap();
        let agent = crate::store::agents::read_agent(&document).unwrap();
        let mut pending = PendingAgentApprovals::default();
        assert!(scan_complete_log_with_snapshot_session(
            ScanCaller::PublicWindowPrefix,
            &self.store,
            &self.id,
            agent.conversation.last_seq,
            &mut session,
            |entry| {
                pending.observe(&entry);
                Ok(())
            }
        )
        .await
        .is_ok());
        pending.into_entries()
    }

    pub(super) fn assert_snapshot_reads(&self) {
        self.assert_read_only();
        let commands = self.commands.lock().unwrap();
        let first = commands.iter().find(|c| c.contains_key("find")).unwrap();
        for c in commands
            .iter()
            .filter(|c| c.contains_key("find") || c.contains_key("getMore"))
        {
            assert_eq!(
                c.get_document("lsid").unwrap(),
                first.get_document("lsid").unwrap()
            );
            if c.contains_key("find") {
                assert!(command_uses_snapshot_read(c));
            }
        }
    }
}

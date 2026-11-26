use super::*;

impl Fixture {
    pub(super) async fn request(
        &self,
        case: &str,
        ingress: usize,
        expected: u16,
        code: Option<&str>,
    ) -> Value {
        self.request_key(
            case,
            ingress,
            expected,
            code,
            &uuid::Uuid::new_v4().to_string(),
        )
        .await
    }

    pub(super) async fn request_key(
        &self,
        case: &str,
        ingress: usize,
        expected: u16,
        code: Option<&str>,
        key: &str,
    ) -> Value {
        let receipt_before = self.control_receipt().await;
        let before = self.h.stored(&self.agent.id).await;
        let entries: usize = self
            .replicas
            .iter()
            .map(|r| r.entered.load(Ordering::SeqCst))
            .sum();
        let before_ownership = self.ownership_io.load(Ordering::SeqCst);
        let start = std::time::Instant::now();
        let response = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(12))
            .build()
            .unwrap()
            .request(
                if self.deleting {
                    Method::DELETE
                } else {
                    Method::POST
                },
                format!(
                    "{}/api/v1/agents/{}{}",
                    self.replicas[ingress].address,
                    self.agent.id,
                    if self.deleting { "" } else { "/stop" }
                ),
            )
            .header("idempotency-key", key)
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let body = response.text().await.unwrap();
        let body: Value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&body).unwrap()
        };
        if case != "concurrent_worker_and_heartbeat" {
            assert_eq!(
                self.ownership_io.load(Ordering::SeqCst),
                before_ownership,
                "public durable admission must not query, claim or mutate owner custody"
            );
        }
        let after = self.h.stored(&self.agent.id).await;
        let lease = self.ownership.owner(&self.agent.id).await.unwrap();
        let row = self
            .admin
            .database(&self.name)
            .collection::<mongodb::bson::Document>("agents")
            .find_one(doc! {"_id":self.agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let log_format = row.get_str("agent_log_format").unwrap_or("legacy");
        assert_eq!(
            self.replicas
                .iter()
                .map(|r| r.entered.load(Ordering::SeqCst))
                .sum::<usize>()
                - entries,
            1,
            "direct admission enters only the ingress handler"
        );
        let receipt_after = self.control_receipt().await;
        if status == 202 {
            let receipt = receipt_after
                .as_ref()
                .expect("202 has a durable control receipt");
            assert!(receipt.get_i64("scheduled_revision").unwrap() as u64 <= after.revision);
            if receipt_before.is_some() {
                assert_eq!(receipt_after, receipt_before);
            }
            self.completed_response(key, 202).await;
        } else if status == 503 || status == 409 {
            assert_eq!(
                receipt_after, receipt_before,
                "refusal must not invent admission"
            );
        }
        let value = json!({"case":case,"method":if self.deleting {"DELETE"} else {"STOP"},"ingress":if ingress==self.owner {"owner"} else {"non_owner"}, "status":status,"body":body,"elapsed_ms":start.elapsed().as_millis(),"handler_entries":self.replicas.iter().map(|r|r.entered.load(Ordering::SeqCst)).sum::<usize>()-entries,"before_revision":before.revision,"after_revision":after.revision,"status_after":format!("{:?}",after.status),"work_after":format!("{:?}",after.work.step),"durable_lease_live":lease.is_some(),"epoch":lease.map(|l|l.token.epoch),"original_authority_cancelled":self.authority.receiver_cancelled(),"failed_mongo_commands":self.failed_commands.load(Ordering::SeqCst),"log_format":log_format});
        println!("AM503_RESULT {value}");
        if expected != 0 {
            assert_eq!(status, expected, "{value}");
        }
        if let Some(code) = code {
            assert_eq!(body["code"], code, "{value}");
        }
        value
    }
    pub(super) async fn complete(&self) {
        for _ in 0..8 {
            for replica in &self.replicas {
                let result = replica
                    .worker
                    .run_one(&self.agent.id, SystemTime::now())
                    .await;
                assert!(
                    result.is_ok()
                        || matches!(
                            result,
                            Err(layer2_core::sandbox::store::SandboxStoreError::Conflict)
                        ),
                    "unexpected worker error: {result:?}"
                );
            }
            let agent = self.h.stored(&self.agent.id).await;
            if agent.work.step == AgentWorkStep::None
                && agent.status
                    == if self.deleting {
                        AgentStatus::Deleted
                    } else {
                        AgentStatus::Stopped
                    }
            {
                let requests = self.h.backend.requests().await;
                let close = requests
                    .iter()
                    .filter(|(_, r)| r.operation() == WireOperation::CloseAgent)
                    .count();
                let home = requests
                    .iter()
                    .filter(|(_, r)| r.operation() == WireOperation::RemoveAgentHome)
                    .count();
                let worktree = requests
                    .iter()
                    .filter(|(_, r)| r.operation() == WireOperation::RemoveWorktree)
                    .count();
                assert_eq!(close, 1, "no duplicate CloseAgent effect");
                assert_eq!(
                    home,
                    usize::from(self.deleting),
                    "no duplicate RemoveAgentHome effect"
                );
                assert_eq!(
                    worktree,
                    usize::from(self.deleting),
                    "no duplicate RemoveWorktree effect"
                );
                println!(
                    "AM503_EFFECTS {}",
                    json!({"close":close,"remove_home":home,"remove_worktree":worktree})
                );
                println!(
                    "AM503_COMPLETION {}",
                    json!({"method":if self.deleting {"DELETE"} else {"STOP"},"status":format!("{:?}",agent.status),"work":"None","revision":agent.revision})
                );
                return;
            }
        }
        panic!(
            "cleanup did not complete: {:?}",
            self.h.stored(&self.agent.id).await.work.step
        );
    }
    pub(super) async fn assert_pending_while_cancelled(&self, case: &str) {
        let before = self.h.stored(&self.agent.id).await;
        for replica in &self.replicas {
            let result = replica
                .worker
                .run_one(&self.agent.id, SystemTime::now())
                .await;
            assert!(
                matches!(
                    result,
                    Ok(false) | Err(layer2_core::sandbox::store::SandboxStoreError::Conflict)
                ),
                "cancelled worker unexpectedly progressed: {result:?}"
            );
        }
        let after = self.h.stored(&self.agent.id).await;
        assert_eq!(before.revision, after.revision);
        assert_ne!(after.work.step, AgentWorkStep::None);
        println!(
            "AM503_PENDING {}",
            json!({"case":case,"method":if self.deleting {"DELETE"} else {"STOP"},"status":format!("{:?}",after.status),"work":format!("{:?}",after.work.step),"no_worker_progress":true,"revision":after.revision})
        );
    }
    pub(super) async fn acquire_successor_for_worker(&self) {
        assert!(
            self.ownership
                .owner(&self.agent.id)
                .await
                .unwrap()
                .is_none(),
            "public admission must not claim ownership on develop"
        );
        let successor = self.replicas[self.owner]
            .coordinator
            .acquire(&self.agent.id)
            .await
            .unwrap()
            .unwrap();
        assert!(successor.token.epoch > self.authority.token.epoch);
        println!(
            "AM503_WORKER_ACQUIRE {}",
            json!({"epoch":successor.token.epoch,"public_requests_did_not_acquire":true})
        );
    }
    pub(super) async fn close(self) {
        measurements::settle(&self.usage).await;
        self.usage.lock().unwrap().report(self.deleting);
        self.admin.database(&self.name).drop().await.unwrap();
    }
    pub(super) async fn fail_commands(
        &self,
        data: mongodb::bson::Document,
        mode: mongodb::bson::Bson,
    ) {
        let mut data = data;
        data.insert("appName", &self.app_name);
        self.admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand", "mode":mode, "data":data})
            .await
            .unwrap();
    }
    pub(super) async fn disable_failpoint(&self) {
        self.admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
    }
}

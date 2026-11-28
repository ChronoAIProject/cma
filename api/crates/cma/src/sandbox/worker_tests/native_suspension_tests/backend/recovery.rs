use super::*;
impl NativeBackend {
    pub async fn lose_object(&self) {
        self.row.lock().await.take();
    }
    pub async fn set_lifecycle(&self, state: &str) {
        self.row
            .lock()
            .await
            .as_mut()
            .unwrap()
            .observation
            .lifecycle
            .state = state.into();
    }

    pub async fn stage_reserved_request(&self, request: ProviderSandboxRequest) {
        let mut rows = self.row.lock().await;
        let row = rows.as_mut().unwrap();
        assert_eq!(row.request.image, request.image);
        assert_eq!(row.request.volumes, request.volumes);
        row.observation.generation = request.metadata["cma/runtime-generation"].parse().unwrap();
        row.request = request;
        // The retained provider object and failed Pod have not been replaced.
    }

    pub async fn restart_process(&self) {
        self.retirement_incarnation.fetch_add(1, Ordering::SeqCst);
        let rows = self.row.lock().await;
        let pod = rows.as_ref().unwrap().pod.as_ref().unwrap();
        self.inner.restart_persistent_process(pod).await.unwrap();
    }

    pub(super) async fn run_recovery_fixture(
        &self,
        _: &RuntimeHandle,
        _command: &layer3_adapter::hosted::RuntimeRecoveryCommand,
    ) -> layer3_adapter::Result<Vec<u8>> {
        use serde_json::json;
        self.calls.push("inspect-history");
        if let Some(response) = self.recovery_response.lock().unwrap().clone() {
            return Ok(response);
        }
        let code = *self.recovery_code.lock().unwrap();
        if code == Some("unavailable") {
            return Err(layer3_adapter::AdapterError::Unavailable(
                "private provider detail".into(),
            ));
        }
        let malformed = matches!(
            code,
            Some("plan_missing" | "plan_mismatch" | "digest_mismatch" | "history_set")
        );
        if let Some(code) = code.filter(|_| !malformed) {
            return Ok(serde_json::to_vec(
                &json!({"version": 1, "status": "refused", "code": code}),
            )
            .unwrap());
        }
        let mut plan = self.recovery_plan.lock().unwrap().clone();
        if plan.is_null() {
            return Err(layer3_adapter::AdapterError::Unsupported(
                "fixture history not configured".into(),
            ));
        }
        if code == Some("plan_mismatch") {
            plan["runtime"]["generation"] = json!(42);
        }
        if code == Some("history_set") {
            plan["histories"] = json!([]);
        }
        let digest =
            layer2_core::sandbox::SandboxUnpublishedRecoveryProof::provider_plan_digest(&plan)
                .unwrap();
        if self.replace_after_history.swap(false, Ordering::SeqCst) {
            self.restart_process().await;
        }
        let mut output =
            json!({"version": 1, "status": "inspected", "plan": plan, "planSha256": digest});
        if code == Some("plan_missing") {
            output.as_object_mut().unwrap().remove("plan");
        }
        if code == Some("digest_mismatch") {
            output["planSha256"] = json!("0".repeat(64));
        }
        Ok(serde_json::to_vec(&output).unwrap())
    }
}

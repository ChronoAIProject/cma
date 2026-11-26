use super::*;

#[derive(Default)]
pub(in super::super) struct RetirementProbePause {
    pub entered: Notify,
    pub release: Notify,
}

impl NativeBackend {
    pub(super) fn check_retirement_fixture_pod(
        &self,
        pod: &layer3_adapter::RetirementPod,
    ) -> layer3_adapter::Result<()> {
        if *pod != super::super::retirement_case6::records::pod()
            || self.retirement_incarnation.load(Ordering::SeqCst) != 0
        {
            return Err(layer3_adapter::AdapterError::Rejected(
                "receiver changed".into(),
            ));
        }
        Ok(())
    }

    pub(super) async fn challenge_retirement_fixture(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        receiver: &layer3_adapter::receiver_failure::RetirementReceiver,
    ) -> layer3_adapter::Result<bool> {
        self.check_retirement_fixture_pod(&receiver.pod)?;
        if self
            .row
            .lock()
            .await
            .as_ref()
            .is_some_and(|row| row.observation.lifecycle.state == "suspended")
        {
            return Err(absent());
        }
        if &receiver.runtime != handle || self.request().await.metadata != request.metadata {
            return Err(layer3_adapter::AdapterError::Rejected(
                "runtime changed".into(),
            ));
        }
        let pause = self.retirement_probe_pause.lock().unwrap().take();
        if let Some(pause) = pause {
            pause.entered.notify_one();
            pause.release.notified().await;
        }
        let url = self
            .retirement_receiver_url
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(absent)?;
        let response = reqwest::Client::new()
            .post(format!(
                "{url}/runtime/agents/{}/manager-ownership/challenge",
                receiver.owner.agent_id
            ))
            .header("OPEN-SANDBOX-API-KEY", "local-fixture-only")
            .json(&receiver.owner)
            .send()
            .await
            .map_err(|_| absent())?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(true)
        } else if response.status().is_success() {
            Ok(false)
        } else {
            Err(absent())
        }
    }

    pub(super) async fn retire_verified_fixture(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &layer3_adapter::HomeVolumeAttachment,
        pod: Option<&layer3_adapter::RetirementPod>,
    ) -> layer3_adapter::Result<()> {
        if let Some(pod) = pod {
            self.check_retirement_fixture_pod(pod)?;
        } else {
            if self
                .row
                .lock()
                .await
                .as_ref()
                .is_none_or(|row| row.observation.lifecycle.state != "suspended")
            {
                return Err(absent());
            }
            self.calls.push("retire-suspended");
        }
        self.calls.push("retire-alternative");
        let homes = self
            .retirement_home
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(absent)?;
        homes.verify(home).await?;
        {
            let rows = self.row.lock().await;
            if let Some(row) = rows.as_ref() {
                if row.observation.handle != *handle || row.request != *request {
                    return Err(layer3_adapter::AdapterError::Rejected(
                        "fixture retirement identity changed".into(),
                    ));
                }
            }
        }
        self.teardown_retained(handle).await
    }
    pub(super) async fn verify_retired_fixture(
        &self,
        _: &RuntimeHandle,
        _: &layer3_adapter::HomeVolumeAttachment,
    ) -> layer3_adapter::Result<()> {
        self.calls.push("verify-retired");
        if self.hold_finality.load(Ordering::SeqCst) || self.row.lock().await.is_some() {
            return Err(absent());
        }
        Ok(())
    }
}

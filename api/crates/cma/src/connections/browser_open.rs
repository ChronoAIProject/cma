//! Open presentation readiness is distinct from fresh action verification.
use super::{
    BrowserConnections, Caller, CheckRequest, ConnectionError, ConnectionReport, ConnectionRow,
    ConnectionTarget, Serialize, ServiceRequirementState, CODEX_SUBSCRIPTION_CONNECTION,
    CONNECTION_SERVICES, PUBLIC_CODEX_SERVICE,
};

#[derive(Serialize)]
pub(super) struct OpenReport {
    #[serde(flatten)]
    pub(super) report: ConnectionReport,
    pub(super) verification: &'static str,
    pub(super) valid_for_ms: u64,
}

impl BrowserConnections {
    pub(super) async fn open(
        &self,
        caller: &Caller,
        request: CheckRequest,
    ) -> Result<OpenReport, ConnectionError> {
        if !matches!(&request.target, ConnectionTarget::Agent { .. }) || request.active.is_some() {
            return Err(ConnectionError::Invalid);
        }
        // Identity and exact target/service scope are always authorized freshly.
        let services = self.targets.services(caller, &request.target).await?;
        if let Some(cached) = self.readiness.get(&caller.subject, &services) {
            self.readiness.refresh(
                self.broker.clone(),
                caller.subject.clone(),
                services.clone(),
            );
            let rows = services
                .into_iter()
                .map(|service| {
                    let is_model = CONNECTION_SERVICES.contains(service.as_str());
                    let display_name = match service.as_str() {
                        CODEX_SUBSCRIPTION_CONNECTION => "My Codex subscription",
                        PUBLIC_CODEX_SERVICE => "Chrono LLM (public)",
                        _ => "Required NyxID service",
                    }
                    .to_owned();
                    ConnectionRow {
                        service,
                        display_name,
                        kind: if is_model { "codex" } else { "nyxid" },
                        state: ServiceRequirementState::Satisfied,
                        connection_mode: "unknown",
                    }
                })
                .collect();
            return Ok(OpenReport {
                report: ConnectionReport {
                    owner: caller.subject.to_string(),
                    ready: true,
                    checked_at: cached.checked_at,
                    services: rows,
                    active_state: None,
                },
                verification: "cached",
                valid_for_ms: cached.valid_for_ms,
            });
        }
        Ok(OpenReport {
            report: self.check_authorized(caller, request, services).await?,
            verification: "fresh",
            valid_for_ms: 0,
        })
    }
}

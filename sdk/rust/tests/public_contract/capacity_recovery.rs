use super::context::Context;
use cma_sdk::Result;
pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let agents = c.client.agents();
    match c.case.operation.as_str() {
        "GET /api/v1/agents/{id}/capacity-recovery/dry-run" => {
            c.check(agents.capacity_recovery_preflight(c.param("id")).await)
        }
        "GET /api/v1/agents/{id}/capacity-recovery" => {
            c.check(agents.capacity_recovery_status(c.param("id")).await)
        }
        "POST /api/v1/agents/{id}/capacity-recovery/status" => {
            c.check(agents.capacity_recovery_request_status(&c.body()?).await)
        }
        "POST /api/v1/agents/{id}/capacity-recovery/admit" => {
            c.check(agents.admit_capacity_recovery(&c.body()?).await)
        }
        "POST /api/v1/agents/{id}/capacity-recovery/resume" => {
            c.check(agents.resume_capacity_recovery(&c.body()?).await)
        }
        operation => panic!("unmapped recovery operation {operation}"),
    }
}

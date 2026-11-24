use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct ReadinessReport {
    pub ready: bool,
    pub reasons: Vec<&'static str>,
    pub cma_reachable: bool,
}

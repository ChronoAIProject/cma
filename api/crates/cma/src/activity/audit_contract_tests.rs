//! Architectural regressions for the seven explicit audit sites in #1796.
//! Paired with the executable ActivityService wait/error tests, these guards
//! prevent a future broad replacement from routing authority audits to telemetry.
fn compact(source: &str) -> String {
    source.split_whitespace().collect()
}

#[test]
fn explicit_audit_sites_preserve_awaits_and_existing_failure_policies() {
    for (name, source, expected) in [
        ("runtime update", include_str!("../../../layer1-public/src/rest/admin/runtime_update.rs"),
         "state.activity.record(&record).await.map_err(|_|HierarchyProblem::Unavailable)?"),
        ("platform settings", include_str!("../../../layer2-core/src/platform_service.rs"),
         "self.activity.record(&record).await.map_err(|error|CoreError::RuntimeUnavailable(error.to_string()))"),
        ("sandbox operator", include_str!("../../../layer2-core/src/sandbox/admin.rs"),
         "self.activity.record(&record).await.map_err(|error|OperatorError::Unavailable(error.to_string()))"),
        ("gateway records", include_str!("../../../layer2-core/src/gateway_records.rs"),
         "self.activity.record(&record).await.map_err(|error|CoreError::RuntimeUnavailable(error.to_string()))"),
        ("sign-in", include_str!("../../../cma-am-auth/src/auth/records.rs"),
         "ifletErr(error)=records.activity.record(&activity).await{tracing::warn!(error=%error,\"couldnotrecordsuccessfulsign-in\");}"),
        // This site already awaits the ActivityStore port directly before dispatch.
        ("delegation", include_str!("../bot_setups/delegation.rs"),
         "dependencies.activity.append(&audit).await.map_err(|_|BotError::Unavailable)?"),
        ("bot worker", include_str!("../composition/bots.rs"),
         "ifself.activity.record(&record).await.is_err(){tracing::warn!(\"botworkactivitycouldnotbepersisted\");}"),
    ] {
        assert!(compact(source).contains(expected), "{name} must retain its awaited durable audit and failure policy");
    }
}

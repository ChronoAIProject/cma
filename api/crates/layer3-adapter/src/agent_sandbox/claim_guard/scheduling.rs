//! Classify scheduler text at the provider boundary before recording diagnostics.

pub(super) fn report(message: &str) {
    let mut reported = Vec::new();
    let summary = message
        .split_once(". preemption:")
        .map_or(message, |(summary, _)| summary);
    let reasons = summary
        .split_once(": ")
        .filter(|(prefix, _)| prefix.ends_with(" nodes are available"))
        .map_or(summary, |(_, reasons)| reasons);
    for clause in reasons.split(',').map(str::trim) {
        let reason = clause
            .split_once(' ')
            .filter(|(count, _)| count.bytes().all(|byte| byte.is_ascii_digit()))
            .map_or(clause, |(_, reason)| reason.trim_start());
        if let Some(class) =
            classify(reason.trim_end_matches('.')).filter(|class| !reported.contains(class))
        {
            reported.push(class);
            tracing::warn!(parent: None, scheduler_class = class, "Native Claim placement deferred");
        }
    }
}

fn classify(reason: &str) -> Option<&'static str> {
    match reason {
        "Insufficient cpu" => Some("cpu"),
        "Insufficient memory" => Some("memory"),
        "node(s) had volume node affinity conflict"
        | "pod has unbound immediate PersistentVolumeClaims"
        | "node(s) didn't find available persistent volumes to bind"
        | "node(s) did not have enough free storage"
        | "node(s) exceed max volume count" => Some("volume"),
        "node(s) didn't match Pod's node affinity/selector"
        | "node(s) didn't match pod topology spread constraints"
        | "node(s) didn't match pod affinity rules"
        | "node(s) didn't match pod anti-affinity rules"
        | "node(s) didn't satisfy existing pods anti-affinity rules"
        | "node(s) were unschedulable"
        | "node(s) didn't have free ports for the requested pod ports"
        | "no nodes available to schedule pods" => Some("placement"),
        reason if reason.starts_with("node(s) had untolerated taint {") => Some("placement"),
        reason if reason.starts_with("exceeded quota: ") => Some("quota"),
        _ => None,
    }
}

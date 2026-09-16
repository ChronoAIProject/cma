use super::{
    super::sandbox_commands_tests::{context, Fixture},
    Scenario,
};
use axum::Extension;
use layer2_core::{
    resource_access::*,
    sandbox::{agent_io::CommandAuthorizationError, control::*},
};
use serde_json::Value;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let kind = name.strip_prefix("sandbox_command_")?;
    assert!([
        "owner",
        "admin",
        "viewer",
        "member",
        "missing",
        "outage",
        "revoked",
        "no_credential",
        "no_custody",
        "custody_outage",
        "invalid_context",
        "anonymous",
        "queued",
        "applied",
        "rejected"
    ]
    .contains(&kind));
    let f = Fixture::new().await;
    let subject = if kind == "owner" {
        "owner-a"
    } else {
        "recipient"
    };
    let caller = context(subject);
    let mut symbols = std::collections::BTreeMap::new();
    if ["queued", "applied", "rejected"].contains(&kind) {
        let mut record = f.admit(subject).await;
        if kind == "applied" {
            f.worker.run_one(&record.envelope.command_id).await.unwrap();
            record = f
                .store
                .sandbox_control(&record.envelope.command_id)
                .await
                .unwrap()
                .unwrap();
        }
        if kind == "rejected" {
            record = f
                .store
                .reject_sandbox_control(&record, SandboxControlFailure::ConnectionsRequired)
                .await
                .unwrap();
        }
        symbols.insert(
            "command_id".into(),
            Value::String(record.envelope.command_id.to_string()),
        );
        symbols.insert("admitted_at".into(), Value::String(record.admitted_at));
        symbols.insert(
            "queue_deadline".into(),
            Value::String(record.queue_deadline),
        );
        match record.status {
            SandboxControlStatus::Applied { at, .. }
            | SandboxControlStatus::Rejected { at, .. } => {
                symbols.insert("finished_at".into(), Value::String(at));
            }
            SandboxControlStatus::Queued => (),
        }
    }
    match kind {
        "viewer" => *f.authority.0.lock().unwrap() = Ok(OrganizationRole::Viewer),
        "member" => *f.authority.0.lock().unwrap() = Ok(OrganizationRole::Member),
        "missing" => *f.authority.0.lock().unwrap() = Err(MembershipError::NotMember),
        "outage" => *f.authority.0.lock().unwrap() = Err(MembershipError::Unavailable),
        "no_custody" => *f.custody.0.lock().unwrap() = Err(CommandAuthorizationError::Revoked),
        "custody_outage" => {
            *f.custody.0.lock().unwrap() = Err(CommandAuthorizationError::Unavailable)
        }
        "revoked" => f.revoke().await,
        _ => (),
    }
    let mut router = f.router.layer(Extension(if kind == "invalid_context" {
        context("owner-a")
    } else {
        caller.clone()
    }));
    if kind != "no_credential" {
        router = router.layer(Extension(crate::DelegatedCredential::new(
            "request-credential".into(),
        )));
    }
    Some(Scenario {
        router,
        subject: (kind != "anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols,
    })
}

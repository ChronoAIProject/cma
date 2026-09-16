use super::{
    super::{
        resource_access_tests::context,
        shared_agent_tests::{fixture, Reply},
    },
    Scenario,
};
use axum::Extension;
use layer2_core::{resource_access::*, SandboxId, SubjectId};
use std::collections::BTreeMap;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !matches!(
        name,
        "shared_agent_member"
            | "shared_agent_missing"
            | "shared_agent_outage"
            | "shared_agent_revoked"
            | "shared_agent_owner"
            | "shared_agent_owner_outage"
            | "shared_agent_invalid_context"
            | "shared_agent_anonymous"
            | "shared_agent_whole_sandbox"
            | "shared_agent_no_credential"
    ) {
        return None;
    }
    let reply = if name == "shared_agent_missing" {
        Reply::Missing
    } else if name.contains("outage") {
        Reply::Outage
    } else {
        Reply::Member
    };
    let (fixture, access, _, share) = fixture(reply).await;
    if name == "shared_agent_revoked" {
        access
            .change_share(
                &context("owner-a"),
                None,
                ShareCommand {
                    selector: share.selector,
                    change: ShareChange::Revoke { share_id: share.id },
                    expected_share_revision: Some(share.revision),
                    idempotency_key: "revoke".into(),
                },
            )
            .await
            .unwrap();
    }
    if name == "shared_agent_whole_sandbox" {
        access
            .change_share(
                &context("owner-a"),
                Some("request-credential"),
                ShareCommand {
                    selector: ResourceSelector::Sandbox {
                        sandbox_id: SandboxId("sbx_route".into()),
                    },
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::View,
                    },
                    expected_share_revision: None,
                    idempotency_key: "whole-sandbox".into(),
                },
            )
            .await
            .unwrap();
    }
    let subject = if name.contains("owner") || name == "shared_agent_invalid_context" {
        "owner-a"
    } else {
        "recipient"
    };
    let caller = context(subject);
    let attached = if name == "shared_agent_invalid_context" {
        context("different-principal")
    } else {
        caller.clone()
    };
    let mut router = fixture.router.layer(Extension(attached));
    if name != "shared_agent_no_credential" {
        router = router.layer(Extension(crate::DelegatedCredential::new(
            "request-credential".into(),
        )));
    }
    Some(Scenario {
        router,
        subject: (name != "shared_agent_anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols: BTreeMap::new(),
    })
}

use super::{
    super::{
        resource_access_tests::context,
        shared_agent_tests::Reply,
        shared_stream_tests::{fixture, revoke},
    },
    Scenario,
};
use axum::Extension;
use layer2_core::{resource_access::*, Authorization, SandboxId, SubjectId};

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let suffix = name.strip_prefix("shared_stream_")?;
    assert!([
        "member",
        "owner",
        "owner_outage",
        "missing",
        "outage",
        "revoked",
        "no_credential",
        "invalid_context",
        "anonymous",
        "absent_context",
        "owner_absent_context",
        "forbidden",
        "whole"
    ]
    .contains(&suffix));
    let f = fixture().await;
    if suffix == "revoked" || suffix == "whole" {
        revoke(&f).await;
    }
    if suffix == "whole" {
        f.access
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
                    idempotency_key: "whole".into(),
                },
            )
            .await
            .unwrap();
    }
    *f.authority.0.lock().unwrap() = match suffix {
        "missing" => Reply::Missing,
        "outage" | "owner_outage" => Reply::Outage,
        _ => Reply::Member,
    };
    let subject = if suffix.starts_with("owner") || suffix == "invalid_context" {
        "owner-a"
    } else {
        "recipient"
    };
    let mut caller = context(subject);
    if suffix == "forbidden" {
        caller.caller.authorization = Authorization::none();
    }
    let attached = if suffix == "invalid_context" {
        context("foreign")
    } else {
        caller.clone()
    };
    let mut router = crate::agui::router(f.state);
    if !suffix.ends_with("absent_context") {
        router = router.layer(Extension(attached));
    }
    if suffix != "no_credential" {
        router = router.layer(Extension(crate::DelegatedCredential::new(
            "request-credential".into(),
        )));
    }
    Some(Scenario {
        router,
        subject: (suffix != "anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols: Default::default(),
    })
}

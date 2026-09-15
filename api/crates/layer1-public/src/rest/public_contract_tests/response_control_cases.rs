use axum::Extension;
use layer2_core::{sandbox::store::AgentStore, AgentId, RuntimeCapabilities, RuntimeCapability};

use super::Scenario;
use crate::responses::v2::controls::fixtures::{context, fixture};

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("response_controls_v2_")?;
    assert!([
        "owner",
        "foreign",
        "anonymous",
        "unsupported",
        "queued",
        "stale"
    ]
    .contains(&mode));
    let f = fixture(mode != "queued").await;
    if mode == "owner" {
        crate::responses::v2::controls::fixtures::pending_approval(&f, "approval-contract").await;
    }
    if mode == "unsupported" || mode == "stale" {
        let mut agent = f
            .store
            .get_agent(&AgentId("agt_fixture".into()))
            .await
            .unwrap()
            .unwrap();
        let previous = agent.revision;
        if mode == "unsupported" {
            agent.capabilities = RuntimeCapabilities::new([RuntimeCapability::Conversation]);
        }
        agent.revision += 1;
        f.store.save_agent(previous, agent).await.unwrap();
    }
    let subject = if mode == "foreign" {
        "foreign"
    } else {
        "owner-a"
    };
    let caller = context(subject);
    Some(Scenario {
        router: f.router.layer(Extension(caller.clone())).layer(Extension(
            crate::DelegatedCredential::new("fixture-credential".into()),
        )),
        subject: (mode != "anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols: Default::default(),
    })
}

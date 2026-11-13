use layer2_core::admission::AdmissionControlStore;
use layer2_core::{Authorization, Capability, SubjectId};

use super::Scenario;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if let Some(kind) = name
        .strip_prefix("admission_ceilings_")
        .or_else(|| name.strip_prefix("admission_pending_homes_"))
    {
        use crate::rest::admin::tests::admission_control_tests as fixtures;
        let (router, _) = if kind == "conflict" {
            fixtures::admission_fixture_with_ceilings_store(
                false,
                true,
                std::sync::Arc::new(fixtures::PendingHomesFailure::default()),
            )
        } else {
            fixtures::admission_fixture_with_ceilings(false, kind != "closed")
        };
        return Some(Scenario {
            router,
            subject: Some("operator-a"),
            authorization: Authorization::new([Capability::ConfigurePlatform]),
            symbols: Default::default(),
        });
    }
    let kind = name.strip_prefix("admission_control_")?;
    let writes = match kind {
        "read" | "closed" | "history" => false,
        "stop" | "resume" | "invalid" | "conflict" => true,
        _ => return None,
    };
    let (router, store) =
        crate::rest::admin::tests::admission_control_tests::admission_fixture(writes);
    if kind == "resume" || kind == "history" {
        store
            .change(
                0,
                &SubjectId::new("operator-a"),
                "contain incident",
                true,
                100,
            )
            .await
            .unwrap();
    }
    if kind == "history" {
        store
            .change(1, &SubjectId::new("operator-b"), "resolved", false, 101)
            .await
            .unwrap();
    }
    Some(Scenario {
        router,
        subject: Some("operator-a"),
        authorization: Authorization::new([Capability::ConfigurePlatform]),
        symbols: Default::default(),
    })
}

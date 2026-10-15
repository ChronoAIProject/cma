// Uses the same real service/store/router as responses::v2::queued_input::tests.
use axum::Extension;

use super::Scenario;
use crate::responses::v2::controls::fixtures::{context, fixture};

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("queued_input_v2_")?;
    assert!(["owner", "bound", "anonymous", "foreign", "invalid"].contains(&mode));
    let f = fixture(mode == "bound").await;
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

use std::sync::Arc;

use axum::{http::StatusCode, Extension, Router};
use layer2_core::{sandbox::store::CreationFence, Authorization, SubjectId};

use super::Scenario;
use crate::rest::creation_outcomes::*;

const DEPLOYMENT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct Reader(&'static str);

#[async_trait::async_trait]
impl Backend for Reader {
    fn deployment(&self) -> &str {
        DEPLOYMENT
    }

    fn terminal_settlement_enabled(&self) -> bool {
        false
    }

    fn validation_settlement_enabled(&self) -> bool {
        false
    }

    async fn refuse_validation(
        &self,
        _: &Identity<'_>,
        _: ValidationRefusal,
    ) -> Result<(), StatusCode> {
        panic!("a read must never publish a validation refusal");
    }

    async fn lookup(
        &self,
        identity: &Identity<'_>,
        _: &LookupRequest,
    ) -> Result<Outcome, StatusCode> {
        match self.0 {
            "creation_outcome_unavailable" => Err(StatusCode::SERVICE_UNAVAILABLE),
            "creation_outcome_foreign" => Err(StatusCode::FORBIDDEN),
            _ => Ok(Outcome::created(
                &identity.digest(),
                "fixture-generation",
                "sbx_fixture".into(),
                None,
            )),
        }
    }

    async fn begin(&self, _: &Identity<'_>) -> Result<Begin, StatusCode> {
        panic!("a read must never admit a creation");
    }

    async fn refuse_direct(
        &self,
        _: &SubjectId,
        _: &CreationFence,
        _: RefusalCode,
    ) -> Result<bool, StatusCode> {
        panic!("a read must never publish a refusal");
    }
}

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    let name = [
        "creation_outcome_valid",
        "creation_outcome_foreign",
        "creation_outcome_unauthenticated",
        "creation_outcome_unavailable",
        "creation_outcome_bad_input",
        "creation_outcome_wrong_deployment",
    ]
    .into_iter()
    .find(|test| *test == name)?;
    let subject = (name != "creation_outcome_unauthenticated").then_some("owner-fixture");
    let router: Router = if let Some(subject) = subject {
        router(Arc::new(Reader(name))).layer(Extension(AuthorizedLookup(SubjectId::new(subject))))
    } else {
        router(Arc::new(Reader(name)))
    };
    Some(Scenario {
        router,
        subject,
        authorization: Authorization::none(),
        symbols: Default::default(),
    })
}

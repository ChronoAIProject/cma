//! Route finite command mutations; never put a client stream behind the peer timeout.
use super::*;
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Extension, Json,
};
use layer2_core::{resource_access::CallerContext, sandbox::agent_io::*};
use std::sync::OnceLock;

#[cfg(test)]
mod codec_tests;
mod errors;
mod intent;
#[cfg(test)]
mod mongo_tests;
mod origin;
mod reply;
#[cfg(test)]
mod tests;
mod transport;
mod validation;
use intent::{Intent, Source};
pub(super) use origin::Origin;
use reply::Reply;

const CALL_BOUND: Duration = Duration::from_secs(60);
const MAX_REPLY: usize = 128 * 1024;

/// Composed before the command service and connected before the server starts.
/// Weak links avoid a service -> router -> service/relay ownership cycle.
pub(crate) struct AdmissionRouter {
    name: &'static str,
    auth: Arc<crate::auth::AuthState>,
    relay: OnceLock<Weak<Relay>>,
    service: OnceLock<Weak<AgentCommandService>>,
}

impl AdmissionRouter {
    pub(crate) fn new(name: &'static str, auth: Arc<crate::auth::AuthState>) -> Arc<Self> {
        Arc::new(Self {
            name,
            auth,
            relay: OnceLock::new(),
            service: OnceLock::new(),
        })
    }
    pub(crate) fn compose(
        self: &Arc<Self>,
        service: Arc<AgentCommandService>,
    ) -> Arc<AgentCommandService> {
        let service = service.with_owner_routing(self.clone());
        assert!(
            self.service.set(Arc::downgrade(&service)).is_ok(),
            "admission service installed twice"
        );
        service
    }
    fn service(&self) -> Result<Arc<AgentCommandService>, CommandAdmissionError> {
        self.service
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(unavailable)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    identity: crate::auth::deferred_commands::OwnerAdmissionIdentity,
    credential: String,
    origin: Option<Origin>,
    source: Source,
    intent: Intent,
}
impl Drop for Call {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.credential.zeroize();
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseEnvelope {
    owner: OwnershipToken,
    request_sha256: String,
    reply: Result<Reply, errors::AdmissionFailure>,
}

impl Relay {
    pub(crate) fn install_admissions(
        self: &Arc<Self>,
        routers: Vec<Arc<AdmissionRouter>>,
    ) -> Result<(), String> {
        let mut names = std::collections::BTreeSet::new();
        for router in &routers {
            if !names.insert(router.name) || router.service.get().is_none() {
                return Err("admission router service is missing or duplicated".into());
            }
            router
                .relay
                .set(Arc::downgrade(self))
                .map_err(|_| "admission relay already installed")?;
        }
        self.admissions
            .set(routers)
            .map_err(|_| "admission routers already installed".into())
    }
    fn admission(&self, name: &str) -> Result<&Arc<AdmissionRouter>, CommandAdmissionError> {
        self.admissions
            .get()
            .and_then(|routers| routers.iter().find(|router| router.name == name))
            .ok_or_else(unavailable)
    }
}

pub(super) fn path(path: &str) -> Option<(&str, &str)> {
    let (agent, suffix) = path
        .strip_prefix("/internal/agent-manager/v1/control/")?
        .split_once('/')?;
    let name = suffix.strip_prefix("admission/")?;
    if agent.is_empty() || !matches!(name, "public" | "media" | "access") {
        return None;
    }
    Some((agent, name))
}

// The private middleware calls this AFTER checking the signed request and live
// sending member, BEFORE an ownerless claim or destination is selected.
pub(super) async fn preflight(relay: &Relay, uri: &axum::http::Uri, body: &[u8]) -> Result<(), ()> {
    let Some((agent, name)) = path(uri.path()) else {
        return Err(());
    };
    let call: Call = serde_json::from_slice(body).map_err(|_| ())?;
    let router = relay.admission(name).map_err(|_| ())?;
    let mutation = call.mutation();
    if mutation.agent_id().as_str() != agent {
        return Err(());
    }
    let context = router.context(&call).await.map_err(|_| ())?;
    router
        .authorize(&context, &call.credential, &mutation)
        .await
        .map_err(|_| ())
}

pub(super) async fn serve(
    State(relay): State<Arc<Relay>>,
    Path((agent, name)): Path<(String, String)>,
    Extension(_peer): Extension<http::VerifiedPeer>,
    body: axum::body::Bytes,
) -> Response {
    let Some(authority) = layer2_core::agent_ownership::execution_authority() else {
        return http::unavailable();
    };
    if authority.token.agent_id.as_str() != agent
        || authority.token.epoch == 0
        || !authority.is_current()
    {
        return http::unavailable();
    }
    let Ok(router) = relay.admission(&name) else {
        return http::unavailable();
    };
    let Ok(call) = serde_json::from_slice::<Call>(&body) else {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    };
    let mutation = call.mutation();
    if mutation.agent_id() != &authority.token.agent_id {
        return http::unavailable();
    }
    let reply = async {
        let context = router.context(&call).await?;
        let service = router.service()?;
        let result = service
            .execute_owner_mutation(
                &context,
                &call.credential,
                call.submission_source(),
                mutation,
            )
            .await;
        router.context(&call).await?;
        result
    };
    let result = match tokio::time::timeout(CALL_BOUND, reply).await {
        Ok(result) => result,
        Err(_) => return http::unavailable(),
    };
    if !authority.is_current() {
        return http::unavailable();
    }
    Json(ResponseEnvelope {
        owner: authority.token.clone(),
        request_sha256: digest(&body),
        reply: result.map(Into::into).map_err(errors::AdmissionFailure),
    })
    .into_response()
}

impl Call {
    fn mutation(&self) -> OwnerCommandMutation {
        self.intent.clone().into()
    }
    fn submission_source(&self) -> CommandSubmissionSource {
        match self.source {
            Source::Api => CommandSubmissionSource::Api,
            Source::UserInterface => CommandSubmissionSource::UserInterface,
        }
    }
}
fn unavailable() -> CommandAdmissionError {
    CommandLedgerError::Unavailable.into()
}
fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

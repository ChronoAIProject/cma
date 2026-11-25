//! Keep Sandbox claims on their worker while executing Agent effects at the owner.
use super::call_failure::CallFailure;
use super::*;
use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Extension, Json,
};
use layer2_core::{
    agent_ownership::{candidates, PROTOCOL_VERSION},
    sandbox::store::{AgentStore, SandboxStore},
};
use layer3_adapter::{
    hosted::HostedControlTransport,
    owner_control::{agent_target, OwnerControlRoute, OwnerControlRouter},
    wire::{RequestFrame, ResponseFrame},
    RuntimeHandle,
};
use std::collections::BTreeMap;

type Controls = Vec<(Arc<OwnerControlRoute>, Arc<dyn HostedControlTransport>)>;
const MAX_REPLY: usize = 16 * 1024 * 1024;

pub(super) struct Maintenance {
    store: Arc<crate::store::MongoSandboxStore>,
    transports: BTreeMap<String, Arc<dyn HostedControlTransport>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Call {
    transport: String,
    handle: RuntimeHandle,
    request: RequestFrame,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    owner: OwnershipToken,
    response: ResponseFrame,
}

impl Relay {
    pub(crate) fn install_controls(
        self: &Arc<Self>,
        store: Arc<crate::store::MongoSandboxStore>,
        controls: Controls,
    ) {
        let transports = controls
            .iter()
            .map(|(route, transport)| (route.name.clone(), transport.clone()))
            .collect();
        assert!(
            self.maintenance
                .set(Maintenance { store, transports })
                .is_ok(),
            "maintenance relay installed twice"
        );
        for (route, _) in controls {
            route.install(self.clone());
        }
    }

    pub(super) async fn control_destination(&self, id: &AgentId) -> Result<Destination, ()> {
        if let Ok(destination) = self.destination(id).await {
            return Ok(destination);
        }
        let membership = bounded(self.coordinator.store.membership())
            .await
            .map_err(|_| ())?;
        membership.require_compatible().map_err(|_| ())?;
        if bounded(self.coordinator.store.owner(id))
            .await
            .map_err(|_| ())?
            .is_some_and(|owner| owner.expires_at_ms > membership.now_ms)
        {
            return Err(());
        }
        let member = candidates(id, &membership, None)
            .map_err(|_| ())?
            .first()
            .copied()
            .cloned()
            .ok_or(())?;
        if member.instance_id == self.coordinator.instance_id {
            self.coordinator
                .acquire(id)
                .await
                .map_err(|_| ())?
                .ok_or(())?;
            return self.destination(id).await.map_err(|_| ());
        }
        // Epoch zero is a signed claim invitation, never execution authority.
        let token = OwnershipToken {
            agent_id: id.clone(),
            instance_id: member.instance_id.clone(),
            epoch: 0,
            protocol: PROTOCOL_VERSION,
        };
        let authority = Authority::new(
            token.clone(),
            tokio::time::Instant::now() + Duration::from_secs(5),
        );
        Ok(Destination {
            member,
            token,
            authority,
        })
    }

    async fn check_lineage(&self, id: &AgentId, handle: &RuntimeHandle) -> Result<bool, ()> {
        let maintenance = self.maintenance.get().ok_or(())?;
        let agent = tokio::time::timeout(Duration::from_secs(2), maintenance.store.get_agent(id))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
        let Some(agent) = agent else {
            if bounded(self.coordinator.store.has_owner_history(id))
                .await
                .map_err(|_| ())?
            {
                return Err(());
            }
            return Ok(false);
        };
        let sandbox = tokio::time::timeout(
            Duration::from_secs(2),
            maintenance.store.get_sandbox(&agent.sandbox_id),
        )
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?
        .ok_or(())?;
        if sandbox
            .runtime
            .is_none_or(|runtime| runtime.runtime_id != handle.as_str())
        {
            return Err(());
        }
        Ok(true)
    }

    async fn control_call(
        &self,
        transport: &str,
        handle: &RuntimeHandle,
        request: &RequestFrame,
        id: &AgentId,
    ) -> Result<ResponseFrame, CallFailure> {
        let _permit = self
            .acquire(capacity::Class::Control)
            .map_err(|_| CallFailure::RelayCapacity)?;
        let destination = self.control_destination(id).await?;
        let reply = if destination.member.instance_id == self.coordinator.instance_id {
            let transport = self
                .maintenance
                .get()
                .and_then(|maintenance| maintenance.transports.get(transport))
                .ok_or(())?;
            let response = destination
                .authority
                .run(transport.exchange(handle, request.clone()))
                .await
                .map_err(|_| ())??;
            Reply {
                owner: destination.token.clone(),
                response,
            }
        } else {
            let call = Call {
                transport: transport.into(),
                handle: handle.clone(),
                request: request.clone(),
            };
            let body = serde_json::to_vec(&call).map_err(|_| ())?;
            if body.len() > crate::REQUEST_BODY_LIMIT_BYTES {
                return Err(CallFailure::Unavailable);
            }
            let uri: axum::http::Uri = format!("{PREFIX}/control/{id}").parse().map_err(|_| ())?;
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("application/json"),
            );
            let headers = self.authentication.signed_headers(
                &self.coordinator.instance_id,
                &destination.token,
                &axum::http::Method::POST,
                &uri,
                headers,
                &body,
            )?;
            let url = format!(
                "{}{}",
                destination.member.relay_address.trim_end_matches('/'),
                uri
            );
            tokio::time::timeout(Duration::from_secs(60), async {
                let mut response = self
                    .client
                    .post(url)
                    .headers(headers)
                    .body(body)
                    .send()
                    .await
                    .map_err(|_| ())?;
                if !response.status().is_success() {
                    return Err(CallFailure::remote(
                        response,
                        layer3_adapter::agent_sandbox::RuntimeWorkloadClass::Control,
                    )
                    .await);
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
                    if chunk.len() > MAX_REPLY.saturating_sub(bytes.len()) {
                        return Err(CallFailure::Unavailable);
                    }
                    bytes.extend_from_slice(&chunk);
                }
                serde_json::from_slice::<Reply>(&bytes).map_err(|_| CallFailure::Unavailable)
            })
            .await
            .map_err(|_| ())??
        };
        if reply.owner.agent_id != *id
            || reply.owner.instance_id != destination.member.instance_id
            || (destination.token.epoch != 0 && reply.owner != destination.token)
        {
            return Err(CallFailure::Unavailable);
        }
        bounded(self.coordinator.store.check(&reply.owner))
            .await
            .map_err(|_| ())?;
        layer2_core::agent_ownership::observe_transport(reply.owner).map_err(|_| ())?;
        Ok(reply.response)
    }
}

#[async_trait::async_trait]
impl OwnerControlRouter for Relay {
    async fn route(
        &self,
        transport: &str,
        handle: &RuntimeHandle,
        request: &RequestFrame,
    ) -> Option<layer3_adapter::Result<ResponseFrame>> {
        let id = AgentId(agent_target(request)?.into());
        match self.check_lineage(&id, handle).await {
            Ok(false) => None,
            Ok(true) => Some(
                self.control_call(transport, handle, request, &id)
                    .await
                    .map_err(|error| error.into_adapter("Agent owner control unavailable")),
            ),
            Err(()) => Some(Err(unavailable())),
        }
    }
}

pub(super) async fn serve(
    State(relay): State<Arc<Relay>>,
    Extension(_peer): Extension<http::VerifiedPeer>,
    Json(call): Json<Call>,
) -> Response {
    let Some(authority) = layer2_core::agent_ownership::execution_authority() else {
        return http::unavailable();
    };
    if agent_target(&call.request) != Some(authority.token.agent_id.as_str())
        || !matches!(
            relay
                .check_lineage(&authority.token.agent_id, &call.handle)
                .await,
            Ok(true)
        )
    {
        return http::unavailable();
    }
    let Some(transport) = relay
        .maintenance
        .get()
        .and_then(|maintenance| maintenance.transports.get(&call.transport))
    else {
        return http::unavailable();
    };
    match tokio::time::timeout(
        Duration::from_secs(60),
        transport.exchange(&call.handle, call.request),
    )
    .await
    {
        Ok(Ok(response)) => Json(Reply {
            owner: authority.token.clone(),
            response,
        })
        .into_response(),
        Ok(Err(error)) => call_failure::response(error),
        _ => http::unavailable(),
    }
}

fn unavailable() -> layer3_adapter::AdapterError {
    layer3_adapter::AdapterError::Unavailable("Agent owner control unavailable".into())
}

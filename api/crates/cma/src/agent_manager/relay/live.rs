use super::*;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Method, Uri},
    response::{IntoResponse, Response},
    Extension,
};
use futures_util::StreamExt;
use layer2_core::sandbox::{
    AgentLiveFragment, AgentLivePublisher, AgentLiveSource, AgentLiveSubscription,
};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    owner: OwnershipToken,
    fragment: Option<AgentLiveFragment>,
}

struct LiveSource(Arc<Relay>);

impl AgentLiveSource for LiveSource {
    fn subscribe(&self, id: &AgentId) -> Option<AgentLiveSubscription> {
        let permit = self.0.slots.clone().try_acquire_owned().ok()?;
        let (publisher, subscription) = AgentLiveSubscription::channel();
        let relay = self.0.clone();
        let id = id.clone();
        tokio::spawn(async move {
            let _permit = permit;
            tokio::select! {
                _ = publisher.closed() => (),
                _ = relay.follow(id, &publisher) => (),
            }
        });
        Some(subscription)
    }
}

impl Relay {
    pub(crate) fn live_source(self: &Arc<Self>) -> Arc<dyn AgentLiveSource> {
        Arc::new(LiveSource(self.clone()))
    }

    #[cfg(test)]
    pub(super) fn subscribe(self: &Arc<Self>, id: &AgentId) -> Option<AgentLiveSubscription> {
        self.live_source().subscribe(id)
    }

    async fn follow(&self, id: AgentId, publisher: &AgentLivePublisher) {
        while self.coordinator.is_ready() {
            if let Ok(destination) = self.destination(&id).await {
                let authority = destination.authority.clone();
                if destination.member.instance_id == self.coordinator.instance_id {
                    if let Some(updates) = self.updates.upgrade() {
                        let mut lane = updates.subscribe_local(&id);
                        loop {
                            tokio::select! {
                                biased;
                                _ = authority.revoked() => break,
                                fragment = lane.recv() => match fragment {
                                    Ok(fragment) => publisher.publish(fragment, authority.clone()),
                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => publisher.gap(),
                                    Err(_) => break,
                                },
                            }
                        }
                    }
                } else {
                    let _ = self.follow_remote(destination, publisher).await;
                    authority.revoke();
                }
                publisher.gap();
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn follow_remote(
        &self,
        destination: Destination,
        publisher: &AgentLivePublisher,
    ) -> Result<(), ()> {
        let uri: Uri = format!("{PREFIX}/live/{}", destination.token.agent_id)
            .parse()
            .map_err(|_| ())?;
        let proof = self.authentication.sign(
            &self.coordinator.instance_id,
            &destination.token,
            &Method::GET,
            &uri,
            &HeaderMap::new(),
            &[],
        )?;
        let url = format!(
            "{}{}",
            destination.member.relay_address.trim_end_matches('/'),
            uri
        );
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            self.client.get(url).header(PROOF, proof).send(),
        )
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
        if !response.status().is_success() {
            return Err(());
        }
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        loop {
            let chunk = tokio::select! {
                biased;
                _ = destination.authority.revoked() => return Err(()),
                chunk = tokio::time::timeout(Duration::from_secs(5), stream.next()) => chunk.map_err(|_| ())?.ok_or(())?.map_err(|_| ())?,
            };
            for byte in chunk {
                if buffer.len() >= MAX_FRAME {
                    return Err(());
                }
                if byte != b'\n' {
                    buffer.push(byte);
                    continue;
                }
                let frame: Frame = serde_json::from_slice(&buffer).map_err(|_| ())?;
                buffer.clear();
                if frame.owner != destination.token || !self.coordinator.is_ready() {
                    return Err(());
                }
                bounded(self.coordinator.store.check(&frame.owner))
                    .await
                    .map_err(|_| ())?;
                if let Some(fragment) = frame.fragment {
                    if fragment.agent_id != destination.token.agent_id
                        || fragment.delta.len() > 16 * 1024
                    {
                        return Err(());
                    }
                    publisher.publish(fragment, destination.authority.clone());
                }
            }
        }
    }
}

pub(super) async fn serve(
    State(relay): State<Arc<Relay>>,
    Path(agent): Path<String>,
    Extension(_peer): Extension<http::VerifiedPeer>,
) -> Response {
    let Some(authority) = layer2_core::agent_ownership::execution_authority() else {
        return http::unavailable();
    };
    if authority.token.agent_id.as_str() != agent {
        return http::unavailable();
    }
    let Some(updates) = relay.updates.upgrade() else {
        return http::unavailable();
    };
    let Ok(permit) = relay.slots.clone().try_acquire_owned() else {
        return http::unavailable();
    };
    let subscription = updates.subscribe_local(&authority.token.agent_id);
    let stream = futures_util::stream::unfold(
        (subscription, authority, permit),
        |(mut subscription, authority, permit)| async move {
            let fragment = tokio::select! {
                biased;
                _ = authority.revoked() => return None,
                next = subscription.recv() => match next { Ok(fragment) => Some(fragment), Err(_) => return None },
                _ = tokio::time::sleep(Duration::from_secs(1)) => None,
            };
            if !authority.is_current() {
                return None;
            }
            let mut frame = serde_json::to_vec(&Frame {
                owner: authority.token.clone(),
                fragment,
            })
            .ok()?;
            if frame.len() >= MAX_FRAME {
                return None;
            }
            frame.push(b'\n');
            Some((
                Ok::<_, std::io::Error>(frame),
                (subscription, authority, permit),
            ))
        },
    );
    (
        [
            ("content-type", "application/x-ndjson"),
            ("cache-control", "no-store"),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

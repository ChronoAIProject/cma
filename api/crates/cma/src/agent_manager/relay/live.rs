use super::*;
use axum::{
    body::Body,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, Method, Uri},
    response::{IntoResponse, Response},
    Extension,
};
use futures_util::StreamExt;
use layer2_core::sandbox::{
    AgentLiveError, AgentLiveFragment, AgentLivePublisher, AgentLiveSource, AgentLiveSubscription,
};
use tokio::{sync::broadcast::error::RecvError, time::Instant};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    owner: OwnershipToken,
    fragment: Option<AgentLiveFragment>,
    #[serde(default)]
    head_changed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    control: Option<Control>,
}

const LIVE_CONTROL: &str = "live_control=1";

#[derive(serde::Serialize, serde::Deserialize)]
enum Control {
    Retired,
    Lagged(u64),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum LiveExit {
    CleanRetirement,
    BufferedRetirement,
    AuthorityUnavailable,
    OwnerUnavailable,
    HeadUnavailable,
    PeerAdmission,
    PeerEof,
    PeerPartialFrame,
    PeerTransport,
    PeerTimeout,
    InvalidFrame,
}

tokio::task_local! {
    static READ_ADMISSION: AgentId;
}

pub(crate) async fn admitted_observation<F: std::future::Future>(
    id: AgentId,
    future: F,
) -> F::Output {
    READ_ADMISSION.scope(id, future).await
}

struct LiveSource(Arc<Relay>);

impl AgentLiveSource for LiveSource {
    fn subscribe(&self, id: &AgentId) -> Option<AgentLiveSubscription> {
        self.try_subscribe(id).ok().flatten()
    }
    fn try_subscribe(&self, id: &AgentId) -> Result<Option<AgentLiveSubscription>, AgentLiveError> {
        // Read custody already admitted this response through the relay. Its
        // body retains that one observer permit; the local hint lane adds none.
        if let Ok(admitted) = READ_ADMISSION.try_with(Clone::clone) {
            if &admitted != id
                || layer2_core::sandbox::agent_observation::observation_agent().as_ref() != Some(id)
            {
                return Err(AgentLiveError::Unavailable);
            }
            return Ok(self
                .0
                .updates
                .upgrade()
                .map(|updates| updates.subscribe_local(id).with_lag_limit(200)));
        }
        let permit = self
            .0
            .acquire(capacity::Class::Observer)
            .map_err(|_| AgentLiveError::Capacity)?;
        let (publisher, subscription) = AgentLiveSubscription::channel();
        let (ready, admission) = tokio::sync::oneshot::channel();
        let relay = self.0.clone();
        let id = id.clone();
        tokio::spawn(async move {
            let _permit = permit;
            tokio::select! {
                _ = publisher.closed() => (),
                _ = relay.follow(id, &publisher, Some(ready)) => (),
            }
        });
        Ok(Some(subscription.with_admission_deadline(
            admission,
            LIVE_ADMISSION_DEADLINE,
        )))
    }
}

type Admission = Option<tokio::sync::oneshot::Sender<Result<(), AgentLiveError>>>;
fn admitted(admission: &mut Admission, result: Result<(), AgentLiveError>) {
    if let Some(admission) = admission.take() {
        let _ = admission.send(result);
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

    async fn follow(&self, id: AgentId, publisher: &AgentLivePublisher, mut admission: Admission) {
        while self.coordinator.is_ready() {
            if let Ok(destination) = self.destination(&id).await {
                let authority = destination.authority.clone();
                let exit = if destination.member.instance_id == self.coordinator.instance_id {
                    self.follow_local(&authority, publisher, &mut admission)
                        .await
                } else {
                    let exit = self
                        .follow_remote(destination, publisher, &mut admission)
                        .await;
                    authority.revoke();
                    exit
                };
                tracing::debug!(
                    agent = %id,
                    owner_epoch = authority.token.epoch,
                    local = authority.token.instance_id == self.coordinator.instance_id,
                    reason = ?exit,
                    "Agent live relay exited"
                );
                match exit {
                    LiveExit::CleanRetirement => publisher.interrupted(
                        authority.token.clone(),
                        Instant::now() + LIVE_ADMISSION_DEADLINE,
                    ),
                    _ => publisher.gap(),
                }
            }
            if admission.is_some() {
                admitted(&mut admission, Ok(()));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn follow_local(
        &self,
        authority: &Arc<Authority>,
        publisher: &AgentLivePublisher,
        admission: &mut Admission,
    ) -> LiveExit {
        let Some(updates) = self.updates.upgrade() else {
            return LiveExit::OwnerUnavailable;
        };
        let mut lane = updates.subscribe_local(&authority.token.agent_id);
        publisher.attached(authority.clone(), self.coordinator.clone());
        admitted(admission, Ok(()));
        loop {
            tokio::select! {
                biased;
                _ = authority.revoked() => return retirement(&lane),
                fragment = lane.recv() => match fragment {
                    Ok(fragment) if fragment.is_current_for(&authority.token) => {
                        publisher.publish(fragment, authority.clone());
                    }
                    Ok(_) => publisher.gap(),
                    Err(RecvError::Lagged(count)) => publisher.lagged(count),
                    Err(RecvError::Closed) => return LiveExit::OwnerUnavailable,
                },
            }
        }
    }

    pub(super) async fn follow_remote(
        &self,
        destination: Destination,
        publisher: &AgentLivePublisher,
        admission: &mut Admission,
    ) -> LiveExit {
        let Ok(uri) = format!(
            "{PREFIX}/live/{}?{LIVE_CONTROL}",
            destination.token.agent_id
        )
        .parse::<Uri>() else {
            return LiveExit::PeerAdmission;
        };
        let Ok(headers) = self.authentication.signed_headers(
            &self.coordinator.instance_id,
            &destination.token,
            &Method::GET,
            &uri,
            HeaderMap::new(),
            &[],
        ) else {
            return LiveExit::PeerAdmission;
        };
        let url = format!(
            "{}{}",
            destination.member.relay_address.trim_end_matches('/'),
            uri
        );
        let response = match tokio::time::timeout(
            LIVE_ADMISSION_DEADLINE,
            self.client.get(url).headers(headers).send(),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => return LiveExit::PeerTransport,
            Err(_) => return LiveExit::PeerTimeout,
        };
        if !response.status().is_success() {
            let error = if response.status() == axum::http::StatusCode::TOO_MANY_REQUESTS {
                metrics::counter!("cma_relay_refusals_total", "class" => "observer", "reason" => "remote_capacity").increment(1);
                AgentLiveError::Capacity
            } else {
                AgentLiveError::Unavailable
            };
            admitted(admission, Err(error));
            return LiveExit::PeerAdmission;
        }
        // A successful response opens an owner subscription. The reader
        // rechecks the exact lease before treating this as a successor.
        publisher.attached(destination.authority.clone(), self.coordinator.clone());
        admitted(admission, Ok(()));
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        loop {
            let chunk = tokio::select! {
                biased;
                _ = destination.authority.revoked() => return LiveExit::AuthorityUnavailable,
                chunk = tokio::time::timeout(Duration::from_secs(5), stream.next()) => match chunk {
                    Err(_) => return LiveExit::PeerTimeout,
                    Ok(None) if !buffer.is_empty() => return LiveExit::PeerPartialFrame,
                    Ok(None) => return LiveExit::PeerEof,
                    Ok(Some(Err(_))) => return LiveExit::PeerTransport,
                    Ok(Some(Ok(chunk))) => chunk,
                },
            };
            for (offset, &byte) in chunk.iter().enumerate() {
                if buffer.len() >= MAX_FRAME {
                    return LiveExit::InvalidFrame;
                }
                if byte != b'\n' {
                    buffer.push(byte);
                    continue;
                }
                let Ok(frame) = serde_json::from_slice::<Frame>(&buffer) else {
                    return LiveExit::InvalidFrame;
                };
                buffer.clear();
                if frame.owner != destination.token {
                    return LiveExit::InvalidFrame;
                }
                if matches!(frame.control, Some(Control::Retired)) {
                    // A retirement control is terminal and carries no data.
                    // The old exact lease need no longer be live at this point.
                    destination.authority.revoke();
                    if frame.fragment.is_some()
                        || frame.head_changed
                        || offset + 1 != chunk.len()
                        || !self.coordinator.is_ready()
                    {
                        return LiveExit::InvalidFrame;
                    }
                    return match tokio::time::timeout(LIVE_ADMISSION_DEADLINE, stream.next()).await
                    {
                        Ok(None) => LiveExit::CleanRetirement,
                        Ok(Some(Ok(_))) => LiveExit::InvalidFrame,
                        Ok(Some(Err(_))) => LiveExit::PeerTransport,
                        Err(_) => LiveExit::PeerTimeout,
                    };
                }
                if !self.confirm_peer(&destination).await {
                    return LiveExit::AuthorityUnavailable;
                }
                if let Some(Control::Lagged(count)) = frame.control {
                    if frame.fragment.is_some() || frame.head_changed || count == 0 {
                        return LiveExit::InvalidFrame;
                    }
                    publisher.lagged(count);
                }
                if frame.head_changed {
                    let Some(updates) = self.updates.upgrade() else {
                        return LiveExit::HeadUnavailable;
                    };
                    if !updates
                        .notify_owned_head(&destination.token.agent_id, &destination.authority)
                    {
                        return LiveExit::AuthorityUnavailable;
                    }
                }
                if let Some(fragment) = frame.fragment {
                    if fragment.agent_id != destination.token.agent_id
                        || fragment.delta.len() > 16 * 1024
                    {
                        return LiveExit::InvalidFrame;
                    }
                    publisher.publish(fragment, destination.authority.clone());
                }
            }
        }
    }

    async fn confirm_peer(&self, destination: &Destination) -> bool {
        let started = Instant::now();
        let Ok(remaining) = bounded(
            self.coordinator
                .store
                .live_lease_remaining_ms(&destination.token),
        )
        .await
        else {
            return false;
        };
        // Time spent in the read consumes the shorter owner/member allowance.
        let Some(deadline) = started.checked_add(Duration::from_millis(remaining)) else {
            return false;
        };
        self.coordinator.is_ready() && destination.authority.renew_until(deadline).is_ok()
    }
}

fn retirement(lane: &AgentLiveSubscription) -> LiveExit {
    if lane.is_empty() {
        LiveExit::CleanRetirement
    } else {
        LiveExit::BufferedRetirement
    }
}

pub(super) async fn serve(
    State(relay): State<Arc<Relay>>,
    Path(agent): Path<String>,
    RawQuery(query): RawQuery,
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
    let subscription = updates.subscribe_local(&authority.token.agent_id);
    let heads = updates.subscribe_local_head(&authority.token.agent_id);
    let control_enabled = query.as_deref() == Some(LIVE_CONTROL);
    let stream = futures_util::stream::unfold(
        Some((subscription, heads, authority)),
        move |state| async move {
            let (mut subscription, mut heads, authority) = state?;
            let mut frame = Frame {
                owner: authority.token.clone(),
                fragment: None,
                head_changed: false,
                control: None,
            };
            tokio::select! {
                biased;
                _ = authority.revoked() => {
                    let exit = retirement(&subscription);
                    tracing::debug!(
                        agent = %authority.token.agent_id,
                        owner_epoch = authority.token.epoch,
                        reason = ?exit,
                        "Agent live relay owner exited"
                    );
                    if !control_enabled || exit != LiveExit::CleanRetirement {
                        return None;
                    }
                    frame.control = Some(Control::Retired);
                    return Some((encode(&frame)?, None));
                }
                next = heads.recv() => { next.ok()?; frame.head_changed = true; }
                next = subscription.recv() => match next {
                    Ok(fragment) if fragment.is_current_for(&authority.token) => frame.fragment = Some((*fragment).clone()),
                    Ok(_) if control_enabled => frame.control = Some(Control::Lagged(1)),
                    Err(RecvError::Lagged(count)) if control_enabled => {
                        frame.control = Some(Control::Lagged(count));
                    }
                    _ => return None,
                },
                _ = tokio::time::sleep(Duration::from_secs(1)) => (),
            }
            if !authority.is_current() {
                return None;
            }
            Some((encode(&frame)?, Some((subscription, heads, authority))))
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

fn encode(frame: &Frame) -> Option<Result<Vec<u8>, std::io::Error>> {
    let mut bytes = serde_json::to_vec(frame).ok()?;
    if bytes.len() >= MAX_FRAME {
        return None;
    }
    bytes.push(b'\n');
    Some(Ok(bytes))
}

//! Recovery of an initial RuntimeStatus connection before a request is sent.
use super::{rejected, AgentSandboxClient, PodBinding};
use crate::{
    provider::ProviderSandboxRequest, AdapterError, HomeVolumeAttachment, Result, RuntimeHandle,
};
use std::{
    cell::Cell,
    future::Future,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::time::Instant;

const RETRY_BUDGET: Duration = Duration::from_secs(3);
const RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(250), Duration::from_secs(1)];

struct InitialStatus {
    home: HomeVolumeAttachment,
    work_deadline: Box<dyn Fn() -> Result<Option<Instant>> + Send + Sync>,
}

struct Binding {
    runtime: RuntimeHandle,
    pod: PodBinding,
    request: ProviderSandboxRequest,
    initial: Arc<InitialStatus>,
    generation: u64,
    request_digest: String,
}

#[derive(Clone, Copy)]
enum Marker {
    Ready {
        at: Instant,
        observed_at: SystemTime,
    },
    Unreadable {
        at: Instant,
        deadline: Instant,
    },
    Invalid,
}

tokio::task_local! {
    static INITIAL_STATUS: Arc<InitialStatus>;
    static BINDING: Arc<Binding>;
    static MARKER: Cell<Option<Marker>>;
}

/// Scope this policy to the first status exchange following native provisioning.
pub async fn with_initial_relay_readiness_retry<T>(
    home: HomeVolumeAttachment,
    work_deadline: impl Fn() -> Result<Option<Instant>> + Send + Sync + 'static,
    status: impl Future<Output = Result<T>>,
) -> Result<T> {
    INITIAL_STATUS
        .scope(
            Arc::new(InitialStatus {
                home,
                work_deadline: Box::new(work_deadline),
            }),
            status,
        )
        .await
}

pub(crate) async fn bind<T>(
    runtime: &RuntimeHandle,
    pod: &PodBinding,
    request: &ProviderSandboxRequest,
    generation: u64,
    request_digest: &str,
    status: impl Future<Output = Result<T>>,
) -> Result<T> {
    let Ok(initial) = INITIAL_STATUS.try_with(Arc::clone) else {
        return status.await;
    };
    if crate::owner_protocol::current_authority().is_some() {
        return status.await;
    }
    if request.metadata.get("cma/home-identity") != Some(&initial.home.identity()?) {
        return Err(rejected("native relay differs from its initial home"));
    }
    BINDING
        .scope(
            Arc::new(Binding {
                runtime: runtime.clone(),
                pod: pod.clone(),
                request: request.clone(),
                initial,
                generation,
                request_digest: request_digest.into(),
            }),
            status,
        )
        .await
}

pub(crate) fn marker_ready() {
    let _ = MARKER.try_with(|marker| {
        marker.set(Some(Marker::Ready {
            at: Instant::now(),
            observed_at: SystemTime::now(),
        }));
    });
}

pub(crate) fn marker_failed(deadline: Instant) {
    let _ = MARKER.try_with(|marker| {
        marker.set(Some(Marker::Unreadable {
            at: Instant::now(),
            deadline,
        }))
    });
}

pub(crate) fn marker_invalid() {
    let _ = MARKER.try_with(|marker| marker.set(Some(Marker::Invalid)));
}

pub(crate) fn clear_marker() {
    let _ = MARKER.try_with(|marker| marker.set(None));
}

pub(crate) async fn verify_reconnect(client: &AgentSandboxClient) -> Result<()> {
    let binding = BINDING.with(Arc::clone);
    (binding.initial.work_deadline)()?;
    let proof = client
        .native_runtime_proof(&binding.runtime, &binding.request, &binding.initial.home)
        .await?;
    if proof.pod() != &binding.pod {
        return Err(rejected("native relay binding changed before retry"));
    }
    (binding.initial.work_deadline)()?;
    Ok(())
}

pub(crate) async fn prepare<T, F>(
    runtime: &RuntimeHandle,
    pod: &PodBinding,
    request_digest: Option<&str>,
    mut open: impl FnMut(bool) -> F,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    let Ok(binding) = BINDING.try_with(Arc::clone) else {
        return open(false).await;
    };
    if binding.runtime != *runtime
        || binding.pod != *pod
        || request_digest != Some(binding.request_digest.as_str())
    {
        return Err(rejected("native relay binding changed before opening"));
    }
    (binding.initial.work_deadline)()?;
    let started = Instant::now();
    let (first, marker) = attempt(open(false)).await;
    binding.record(0, started, marker, &first);
    let (first, deadline) = match (first, marker) {
        (Err(error @ AdapterError::Unavailable(_)), Some(Marker::Unreadable { at, deadline })) => {
            (error, deadline.min(at + RETRY_BUDGET))
        }
        (result, _) => return result,
    };
    for (retry, delay) in RETRY_DELAYS.into_iter().enumerate() {
        let deadline =
            (binding.initial.work_deadline)()?.map_or(deadline, |work| deadline.min(work));
        let next_open = Instant::now() + delay;
        if next_open >= deadline {
            binding.trace(retry + 1, started.elapsed(), "open", "deadline", None);
            return Err(first);
        }
        let result = tokio::time::timeout_at(deadline, async {
            tokio::time::sleep_until(next_open).await;
            attempt(open(true)).await
        })
        .await;
        match result {
            Ok((result, marker)) => {
                binding.record(retry + 1, started, marker, &result);
                if !matches!(
                    (&result, marker),
                    (
                        Err(AdapterError::Unavailable(_)),
                        Some(Marker::Unreadable { .. })
                    )
                ) {
                    (binding.initial.work_deadline)()?;
                    return result;
                }
            }
            Err(_) => {
                binding.trace(retry + 1, started.elapsed(), "open", "deadline", None);
                return Err(first);
            }
        }
    }
    binding.trace(
        RETRY_DELAYS.len(),
        started.elapsed(),
        "open",
        "exhausted",
        None,
    );
    Err(first)
}

async fn attempt<T>(open: impl Future<Output = Result<T>>) -> (Result<T>, Option<Marker>) {
    MARKER
        .scope(Cell::new(None), async {
            let result = open.await;
            (result, MARKER.with(Cell::get))
        })
        .await
}

impl Binding {
    fn record<T>(
        &self,
        attempt: usize,
        started: Instant,
        marker: Option<Marker>,
        result: &Result<T>,
    ) {
        let reason = match marker {
            Some(Marker::Ready { at, observed_at }) if result.is_ok() => {
                self.trace(
                    attempt,
                    at.duration_since(started),
                    "marker",
                    "valid",
                    Some(observed_at),
                );
                return;
            }
            Some(Marker::Unreadable { .. }) => "unreadable",
            Some(Marker::Invalid) => "invalid",
            None | Some(Marker::Ready { .. }) => match result {
                Ok(_) => return,
                Err(error) => stop_reason(error),
            },
        };
        self.trace(attempt, started.elapsed(), "open", reason, None);
    }

    fn trace(
        &self,
        attempt: usize,
        elapsed: Duration,
        phase: &'static str,
        reason: &'static str,
        marker_at: Option<SystemTime>,
    ) {
        let claim_uid = self
            .initial
            .home
            .receipt
            .native_authority
            .as_deref()
            .map(|origin| origin.claim.uid.as_str());
        let marker_at_unix_ms = marker_at.and_then(|at| {
            at.duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .map(|elapsed| elapsed.as_millis() as u64)
        });
        tracing::info!(
            sandbox_id = %self.initial.home.request.sandbox_id,
            claim_uid,
            pod_uid = %self.pod.pod_uid,
            runtime_generation = self.generation,
            attempt, phase, reason,
            elapsed_ms = elapsed.as_millis() as u64,
            marker_at_unix_ms,
            "Initial runtime relay readiness"
        );
    }
}

fn stop_reason(error: &AdapterError) -> &'static str {
    match error {
        AdapterError::Rejected(_) => "rejected",
        AdapterError::RuntimeCapacity(_) => "capacity",
        _ => "unavailable",
    }
}

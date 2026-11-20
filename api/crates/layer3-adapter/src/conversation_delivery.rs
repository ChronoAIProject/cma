use super::*;

pub(super) async fn receive_delivery(
    mut receipt: oneshot::Receiver<ConversationSendOutcome>,
    mut progress: Option<watch::Receiver<u64>>,
    mut history: Option<watch::Receiver<Option<tokio::time::Instant>>>,
    state: &AtomicU8,
    deadline: Duration,
    refreshing: bool,
    started: tokio::time::Instant,
) -> Result<ConversationSendOutcome, &'static str> {
    let mut cursor = progress.as_ref().map_or(0, |value| *value.borrow());
    let limit = if refreshing && progress.is_some() {
        CONTEXT_DELIVERY_LIMIT
    } else {
        deadline
    };
    let mut loading = false;
    let idle = tokio::time::sleep_until(started + deadline);
    let absolute = tokio::time::sleep_until(started + limit);
    tokio::pin!(idle, absolute);
    loop {
        tokio::select! {
            biased;
            result = &mut receipt => {
                return result.map_err(|_| "the conversation input task ended before delivery");
            }
            changed = async {
                match history.as_mut() {
                    Some(value) => value.changed().await,
                    None => std::future::pending().await,
                }
            } => {
                if changed.is_err() {
                    history = None;
                    continue;
                }
                if state.load(Ordering::Acquire) == WRITING {
                    let until = *history.as_mut()
                        .expect("watched history load").borrow_and_update();
                    loading = until.is_some();
                    let now = tokio::time::Instant::now();
                    idle.as_mut().reset(now + idle_window(loading, deadline));
                    absolute.as_mut().reset(until.unwrap_or(now + limit));
                }
            }
            _ = &mut absolute => {
                return Err("the conversation input task exceeded its delivery limit");
            }
            _ = &mut idle => {
                return Err("the conversation input task did not deliver the operation");
            }
            changed = async {
                match progress.as_mut() {
                    Some(value) => value.changed().await,
                    None => std::future::pending().await,
                }
            } => {
                if changed.is_err() {
                    progress = None;
                    continue;
                }
                let next = *progress.as_ref().expect("watched progress").borrow();
                if next > cursor {
                    cursor = next;
                    // Output from another queued operation must not extend a
                    // request that has not passed its authority guard yet.
                    if state.load(Ordering::Acquire) == WRITING && (loading || refreshing) {
                        idle.as_mut().reset(
                            tokio::time::Instant::now() + idle_window(loading, deadline),
                        );
                    }
                }
            }
        }
    }
}

fn idle_window(loading: bool, deadline: Duration) -> Duration {
    if loading {
        RETAINED_HISTORY_IDLE_DEADLINE
    } else {
        deadline
    }
}

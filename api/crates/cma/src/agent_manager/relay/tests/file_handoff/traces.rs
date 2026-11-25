use super::*;
use tracing::instrument::WithSubscriber as _;

#[derive(Clone, Default)]
pub(super) struct RelayLog(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for RelayLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl RelayLog {
    pub(super) fn observe(&self, router: axum::Router) -> axum::Router {
        let writer = self.clone();
        let dispatch = tracing::Dispatch::new(
            tracing_subscriber::fmt()
                .json()
                .with_current_span(false)
                .with_span_list(false)
                .without_time()
                .with_writer(move || writer.clone())
                .finish(),
        );
        router.layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let dispatch = dispatch.clone();
                async move { next.run(request).with_subscriber(dispatch).await }
            },
        ))
    }

    fn events(&self) -> Vec<Value> {
        let bytes = self.0.lock().unwrap().clone();
        serde_json::Deserializer::from_slice(&bytes)
            .into_iter::<Value>()
            .map(Result::unwrap)
            .filter(|event| event["target"] == "cma::agent_manager::file_relay")
            .map(|event| event["fields"].clone())
            .collect()
    }

    pub(super) fn refusals(&self, reason: &str) -> usize {
        self.events()
            .iter()
            .filter(|event| event["reason"] == reason)
            .count()
    }

    pub(super) fn assert_correlated(
        &self,
        owner: &Self,
        operation: &str,
        trigger: &str,
        status: StatusCode,
    ) {
        let events = self.events();
        let outcome = if status.is_success() {
            "recovered"
        } else {
            "refused"
        };
        let result = events
            .iter()
            .rev()
            .find(|event| {
                event["operation"] == operation
                    && event["trigger"] == trigger
                    && event["outcome"] == outcome
                    && event["status"] == status.as_u16()
            })
            .unwrap_or_else(|| panic!("missing recovery outcome: {events:?}"));
        let request_hash = result["request_hash"].as_str().unwrap();
        assert_eq!(request_hash.len(), 64);
        let hop_hash = result["hop_hash"].as_str().unwrap();
        assert_eq!(hop_hash.len(), 64);
        assert!(
            events.iter().any(|event| {
                event["request_hash"] == request_hash
                    && event["hop_hash"] == hop_hash
                    && event["operation"] == operation
                    && event["trigger"] == trigger
                    && event["outcome"] == "started"
            }),
            "{events:?}"
        );
        let owner_events = owner.events();
        assert!(
            owner_events.iter().any(|event| {
                event["operation"] == operation
                    && event["hop_hash"] == hop_hash
                    && (event["reason"] != "admission_closed" || event["request_hash"] == "none")
                    && matches!(
                        event["reason"].as_str(),
                        Some(
                            "admission_closed"
                                | "coordinator_not_ready"
                                | "authority_changed"
                                | "owner_missing"
                                | "owner_member_unavailable"
                        )
                    )
            }),
            "{owner_events:?}"
        );
    }

    pub(super) fn assert_outer_admission_correlated(&self, owner: &Self) {
        let events = self.events();
        let recovered = events
            .iter()
            .rev()
            .find(|event| {
                event["operation"] == "read"
                    && event["trigger"] == "peer_owner_handoff"
                    && event["outcome"] == "recovered"
            })
            .unwrap();
        assert!(owner.events().iter().any(|event| {
            event["reason"] == "admission_closed" && event["hop_hash"] == recovered["hop_hash"]
        }));
        self.assert_correlated(owner, "read", "peer_owner_handoff", StatusCode::OK);
    }
}

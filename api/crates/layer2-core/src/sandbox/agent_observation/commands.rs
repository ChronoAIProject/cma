//! Current command records are batched only while requests are pending. Results
//! are immutable receipt/state observations, never cached permission decisions.
use super::*;
use crate::{ids::AgentResponseId, sandbox::agent_io::AdmittedAgentCommand};

pub(super) struct Request {
    pub id: AgentResponseId,
    pub reply: oneshot::Sender<Result<Arc<AdmittedAgentCommand>, AgentReadError>>,
}
impl AgentObservationService {
    pub(in crate::sandbox) async fn command(
        &self,
        authorization: &AgentEventStreamAuthorization,
        id: &AgentResponseId,
    ) -> Result<Arc<AdmittedAgentCommand>, AgentReadError> {
        if id.as_str().is_empty() || id.as_str().len() > 256 {
            return Err(AgentReadError::NotFound);
        }
        let (reply, receive) = oneshot::channel();
        self.commands
            .try_send(Request {
                id: id.clone(),
                reply,
            })
            .map_err(|_| unavailable())?;
        authorization
            .bounded(async { receive.await.map_err(|_| unavailable())? })
            .await
    }
}
pub(super) async fn run(
    service: Weak<AgentObservationService>,
    mut receive: mpsc::Receiver<Request>,
) {
    while let Some(first) = receive.recv().await {
        tokio::task::yield_now().await;
        let mut cohort = vec![first];
        while cohort.len() < MAX_OBSERVATION_COHORT {
            match receive.try_recv() {
                Ok(request) => cohort.push(request),
                Err(_) => break,
            }
        }
        let Some(service) = service.upgrade() else {
            return;
        };
        let ids: std::collections::BTreeSet<_> =
            cohort.iter().map(|request| request.id.clone()).collect();
        let ids: Vec<_> = ids.into_iter().collect();
        let records =
            tokio::time::timeout(Duration::from_secs(10), service.store.commands(&ids)).await;
        let records = match records {
            Ok(Ok(records)) if records.len() == ids.len() => ids
                .into_iter()
                .zip(records.into_iter().map(|record| record.map(Arc::new)))
                .collect::<BTreeMap<_, _>>(),
            _ => {
                for request in cohort {
                    let _ = request.reply.send(Err(unavailable()));
                }
                continue;
            }
        };
        for request in cohort {
            let result = records
                .get(&request.id)
                .cloned()
                .unwrap_or_else(|| Err(unavailable()));
            let _ = request.reply.send(result);
        }
    }
}

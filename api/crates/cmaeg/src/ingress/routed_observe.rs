//! Project only the committed winning admission into observer and wake hints.
use super::*;
use crate::ingress::routing_store::AdmittedPlan;

impl RoutedIngress {
    pub(super) async fn report_admitted(
        &self,
        request: &LaunchRequest,
        source: &VerifiedIngress,
        observer: &dyn ActivityObserver,
        committed: AdmittedPlan,
    ) -> Result<Option<LaunchOutcome>, Problem> {
        if !matches!(source.quote, crate::ingress::quoted_context::Intake::Legacy) {
            self.observe_quote_receipt(request, source, observer, Some(&committed.activity))
                .await?;
        }
        let primary = committed.targets.first().ok_or_else(|| {
            Problem::new(ProblemCode::Internal, 503, "routed_admission_unavailable")
        })?;
        // Admission can restore a previous winner, including a concurrent
        // request with another local waiter. Never promise an unattached SSE
        // stream or rewrite the winning plan to the retry's ephemeral waiter.
        let sync_waiter_attached = request.event_ref.open(&self.keys, (self.clock)())
            .ok().is_some_and(|opened| matches!(
                (&opened.payload.reply, &primary.launch.event.reply),
                (cmaeg_core::seal::ReplyAddress::Sync { waiter: incoming },
                 cmaeg_core::seal::ReplyAddress::Sync { waiter: retained }) if incoming == retained
            ));
        observer.admitted(&committed.identity.record_id, &committed.activity);
        for target in &committed.targets {
            let mut change = crate::activity::Change::state(
                target.launch.consumer.as_str(),
                crate::activity::ConsumerState::Queued,
                None,
            );
            change.provider = Some(target.launch.provider.slug.to_string());
            change.conversation_id = Some(target.launch.event.conversation_id.to_string());
            change.display_name = request
                .channel
                .definition
                .consumer(Some(&target.launch.consumer))
                .map(|c| c.label.to_owned());
            observer
                .checkpoint(
                    &committed.identity.record_id,
                    &committed.identity.event_id,
                    change,
                )
                .await;
            observer.launched(
                &committed.identity.record_id,
                &committed.identity.event_id,
                &target.launch.consumer,
                LaunchClass::Deferred,
                request.received_at.elapsed(),
            );
        }
        // A full hint queue leaves the manifest authoritative for recovery.
        self.schedule.notify(&committed);
        Ok(Some(LaunchOutcome::Admitted {
            admission_id: committed.admission_id,
            conversation_id: primary.launch.event.conversation_id.clone(),
            sync_waiter_attached,
        }))
    }
}

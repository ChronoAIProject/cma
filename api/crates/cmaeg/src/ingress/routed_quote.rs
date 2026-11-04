//! Restore the winning projection version before sealing or observing context.
use super::*;
use crate::ingress::{
    quoted_context::{Intake, Snapshot},
    routing_store::EventIdentity,
};
impl RoutedIngress {
    pub(crate) async fn whatsapp_quote_batch_conflict(
        &self,
        channel: &crate::channels::resolver::ResolvedChannel,
        source: &cmaeg_core::channel::Source,
        body: &[u8],
    ) -> Result<bool, AdmissionError> {
        for id in super::super::quoted_context::whatsapp_batch_conflicts(body, source) {
            let identity = EventIdentity {
                owner: channel.definition.owner.subject.clone(),
                channel_id: channel.id.clone(),
                record_id: channel.definition.record_id.clone(),
                event_id: crate::whatsapp::webhook::event_id(source, &id)
                    .map_err(|_| AdmissionError::Invalid)?,
            };
            let existing = self.plans.load(&identity).await?;
            let projects_quote = existing
                .as_ref()
                .map_or(self.quoted_context_enabled, |plan| {
                    plan.quote_projection.is_some()
                });
            if projects_quote {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) async fn admit(
        &self,
        request: &LaunchRequest,
        source: VerifiedIngress,
        observer: &dyn ActivityObserver,
        native_interaction: bool,
        message_plan: Option<super::super::routing_plan::RoutedPlan>,
    ) -> Result<Option<LaunchOutcome>, Problem> {
        if source.oversized {
            return Err(admission_problem(AdmissionError::TooLarge));
        }
        // Preserve existing receipt behavior for paths that never project quotes.
        if matches!(source.quote, Intake::Legacy) {
            self.observe_quote_receipt(request, &source, observer, None)
                .await?;
        }
        for attempt in 0..2 {
            let (projected, version) = self.project_quote(request, &source.quote).await?;
            match self
                .admit_once(
                    &projected,
                    source.clone(),
                    observer,
                    native_interaction,
                    message_plan.clone(),
                    version,
                )
                .await
            {
                Err(error) if error.status == 409 && attempt == 0 => continue,
                result => return result,
            }
        }
        unreachable!()
    }
    async fn project_quote(
        &self,
        request: &LaunchRequest,
        intake: &Intake,
    ) -> Result<(LaunchRequest, Option<u8>), Problem> {
        let mut request = request.clone();
        let candidate = match intake {
            Intake::Legacy => return Ok((request, None)),
            Intake::Pinned(snapshot) => return Ok((request, snapshot.as_ref().map(|s| s.version))),
            Intake::Candidate(quote) => quote.clone(),
            Intake::Ambiguous => None,
        };
        if !matches!(request.activity.kind, ActivityKind::Message { .. }) {
            return Ok((request, None));
        }
        let identity = EventIdentity {
            owner: request.channel.definition.owner.subject.clone(),
            channel_id: request.channel.id.clone(),
            record_id: request.channel.definition.record_id.clone(),
            event_id: request.activity.event_id.clone(),
        };
        let existing = self
            .plans
            .load(&identity)
            .await
            .map_err(admission_problem)?;
        let version = existing
            .as_ref()
            .map_or(self.quoted_context_enabled.then_some(1), |p| {
                p.quote_projection
            });
        if version.is_some() && matches!(intake, Intake::Ambiguous) {
            return Err(admission_problem(AdmissionError::Conflict));
        }
        let original = request.activity.clone();
        let snapshot = Snapshot {
            version: 1,
            quoted_message: version.and(candidate),
        };
        snapshot
            .apply(&mut request.activity)
            .map_err(|_| admission_problem(AdmissionError::Invalid))?;
        if original == request.activity {
            return Ok((request, version));
        }
        let mut event = request
            .event_ref
            .open(&self.keys, (self.clock)())
            .map_err(|_| admission_problem(AdmissionError::Invalid))?
            .payload;
        if !event.verifies_activity(&original) {
            return Err(admission_problem(AdmissionError::Invalid));
        }
        event.activity_sha256 = Some(cmaeg_core::seal::event_ref::activity_digest(
            &request.activity,
        ));
        request.event_ref = cmaeg_core::seal::EventRef::seal(&event, &self.keys)
            .map_err(|_| admission_problem(AdmissionError::Invalid))?;
        Ok((request, version))
    }
}

impl RoutedIngress {
    pub(super) async fn observe_quote_receipt(
        &self,
        request: &LaunchRequest,
        source: &VerifiedIngress,
        observer: &dyn ActivityObserver,
        retained: Option<&cmaeg_core::activity::Activity>,
    ) -> Result<(), Problem> {
        let receipt = match (source.received_at, retained) {
            (None, None) => observer.receive(request).await,
            (at, activity) => {
                observer
                    .receive_at(
                        &request.channel.definition.owner.subject,
                        &request.channel.definition.record_id,
                        activity.unwrap_or(&request.activity),
                        at.unwrap_or_else(|| (self.clock)()),
                    )
                    .await
            }
        };
        receipt.map_err(|_| {
            Problem::new(
                ProblemCode::ProviderUnavailable,
                503,
                "channel_activity_unavailable",
            )
        })
    }
}

//! Only actual authenticated Custom ingress can use the immutable Custom opt-in.
use super::LaunchRequest;
use cmaeg_core::{activity::SourceKind, channel::Source};

pub(super) fn opted_in(request: &LaunchRequest) -> bool {
    request.channel.definition.durable_custom_execution
        && matches!(request.activity.source, SourceKind::Custom)
        && matches!(
            request
                .channel
                .definition
                .source_of(&request.activity.source),
            Some(Source::Custom { .. })
        )
}

/// An opted-in source never falls back if durable dependencies are unavailable.
pub(super) fn refusal(request: &LaunchRequest) -> Option<super::LaunchOutcome> {
    (super::ordinary(request) && opted_in(request)).then(|| {
        super::LaunchOutcome::Refused(cmaeg_core::problem::Problem::new(
            cmaeg_core::problem::ProblemCode::ProviderUnavailable,
            503,
            "custom_durable_admission_unavailable",
        ))
    })
}

/// Evaluate only before a fresh admission, never when replaying accepted work.
pub(super) fn validate_media(
    request: &LaunchRequest,
    plan: &crate::ingress::routing_plan::RoutedPlan,
    providers: &std::collections::BTreeSet<String>,
) -> Result<(), super::Problem> {
    if opted_in(request)
        && crate::turns::render_media::attachments(&plan.activity)
            .iter()
            .any(|a| a.source.is_some())
        && plan.targets.iter().any(|t| {
            !t.provider.target.is_profile() && !providers.contains(t.provider.slug.as_str())
        })
    {
        return Err(super::Problem::new(
            cmaeg_core::problem::ProblemCode::ProviderUnavailable,
            503,
            "custom_existing_agent_media_unavailable",
        ));
    }
    Ok(())
}

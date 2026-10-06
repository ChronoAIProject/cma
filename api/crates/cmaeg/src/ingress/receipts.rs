//! Authenticated platform fulfillment evidence; it never represents user consent.
use super::IngressState;
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    activity::{Activity, ActivityKind},
    problem::Problem,
};
use futures_util::future::BoxFuture;

pub trait PlatformReceiptObserver: Send + Sync {
    fn observe<'a>(
        &'a self,
        channel: &'a ResolvedChannel,
        activity: &'a Activity,
    ) -> BoxFuture<'a, Result<(), Problem>>;
}
pub async fn observe(
    state: &IngressState,
    channel: &ResolvedChannel,
    activity: &Activity,
) -> Result<(), Problem> {
    if matches!(&activity.kind, ActivityKind::Event { ce } if ce.r#type == "telegram.managed_bot_created")
    {
        if let Some(observer) = &state.receipt_observer {
            observer.observe(channel, activity).await?;
        }
    }
    Ok(())
}

//! Attach durable history before the activity observer can admit ingress.
use crate::{console::observer::Observers, http::ManagementState};
use std::sync::Arc;

pub(super) async fn configure(
    state: &mut ManagementState,
    observers: &mut Observers,
) -> Result<(), crate::activity::history::Error> {
    crate::activity::history::configure(state).await?;
    observers.activity = Arc::new(crate::activity::ObservedActivity::new(
        state.activity.clone(),
        observers.activity.clone(),
    ));
    Ok(())
}

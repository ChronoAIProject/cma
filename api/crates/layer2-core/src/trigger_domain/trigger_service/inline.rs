use super::{TriggerService, TriggerServiceError};
use crate::{
    trigger_domain::{TriggerContent, TriggerTarget},
    Caller,
};

impl TriggerService {
    pub(super) async fn materialize_inline(
        &self,
        caller: &Caller,
        draft: TriggerContent,
        existing: Option<&TriggerTarget>,
        delegated_credential: Option<&str>,
    ) -> Result<TriggerContent, TriggerServiceError> {
        let Some(materializer) = &self.inline_materializer else {
            return Ok(draft);
        };
        let target = match existing {
            Some(existing) => {
                materializer
                    .materialize_draft_update(
                        &caller.subject,
                        &draft.target,
                        existing,
                        delegated_credential,
                    )
                    .await
            }
            None => {
                materializer
                    .materialize_with_credential(
                        &caller.subject,
                        &draft.target,
                        delegated_credential,
                    )
                    .await
            }
        }
        .map_err(TriggerServiceError::Reference)?;
        Ok(TriggerContent { target, ..draft })
    }
}

use super::ResponsesV2;
use crate::{Result, agents::RetainedOutputPage};
impl ResponsesV2<'_> {
    /// Read retained partial text only after typed discovery on this response's failed turn.
    pub async fn retained_output(
        &self,
        id: &str,
        cursor: Option<&str>,
        availability: Option<&crate::agents::RetainedOutputAvailability>,
    ) -> Result<RetainedOutputPage> {
        if cursor.is_none() && !availability.is_some_and(|value| value.available) {
            return Ok(RetainedOutputPage::default());
        }
        let query: Vec<_> = cursor
            .map(|value| ("cursor", value.to_owned()))
            .into_iter()
            .collect();
        let page: RetainedOutputPage = self
            .get(&["api", "v2", "responses", id, "retained-output"], &query)
            .await?;
        page.validate(cursor, None)?;
        Ok(page)
    }
}

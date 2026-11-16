use super::ResponsesV2;
use crate::{Result, agents::RetainedOutputPage};
impl ResponsesV2<'_> {
    /// Discover and page retained partial text for this exact response's failed turn.
    pub async fn retained_output(
        &self,
        id: &str,
        cursor: Option<&str>,
    ) -> Result<RetainedOutputPage> {
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

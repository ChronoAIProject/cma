use super::{OutputItem, ResponsesV2};
use crate::{Error, Result, resources::identity};
use serde::Deserialize;

string_enum! { /// Bounded response output page tag.
    pub enum OutputPageKind { OutputPage => "cma.response.output_page" }
}
/// One output page; continuation never submits input or changes the response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OutputPage {
    /// Page document tag.
    pub object: OutputPageKind,
    /// Exact durable response identity.
    pub response_id: String,
    /// Original output items in durable order.
    pub data: Vec<OutputItem>,
    /// Whether a continuation is available.
    pub has_more: bool,
    /// Opaque continuation for this response and source format.
    pub next_after: Option<String>,
    /// Captured source boundary when supplied by an indexed reader.
    pub through: Option<u64>,
}

impl ResponsesV2<'_> {
    /// Read one bounded page using the continuation returned for this response.
    pub async fn read_output_page(&self, id: &str, after: Option<&str>) -> Result<OutputPage> {
        let query: Vec<_> = after
            .map(|value| ("after", value.to_owned()))
            .into_iter()
            .collect();
        let page: OutputPage = self
            .get(&["api", "v2", "responses", id, "output"], &query)
            .await?;
        identity(&page.response_id, id)?;
        if page.has_more != page.next_after.is_some()
            || (page.has_more
                && (page.next_after.as_deref() == after
                    || page.next_after.as_deref() == Some("")
                    || page.data.is_empty()))
        {
            return Err(Error::protocol(
                "response output page continuation is inconsistent",
            ));
        }
        Ok(page)
    }
}

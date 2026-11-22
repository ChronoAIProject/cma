use super::*;
use futures_util::TryStreamExt;

impl CaptureStore {
    pub(in crate::store::sandbox_usage_export) async fn measured_delivery_page(
        &self,
        after: &str,
    ) -> Result<Vec<String>, SandboxStoreError> {
        let prefix = keys::prefix("delivery", &self.config.namespace);
        let after = (!after.is_empty()).then(|| format!("{prefix}{after}"));
        let mut cursor = self
            .collection(INVENTORY)
            .find(keys::page(&prefix, after.as_deref())?)
            .sort(doc! {"_id":1})
            .limit(32)
            .await
            .map_err(unavailable)?;
        let mut result = Vec::new();
        while let Some(row) = cursor.try_next().await.map_err(unavailable)? {
            let stream = row.get_str("stream_id").map_err(unavailable)?;
            let source = row.get_str("source_stream").map_err(unavailable)?;
            if row.get_i64("document_schema_version").ok() != Some(1)
                || !uuid(stream)
                || !uuid(source)
                || keys::key(&row)? != format!("{prefix}{stream}")
            {
                return Err(corrupt());
            }
            let metadata = self
                .collection(INVENTORY)
                .find_one(doc! {"_id":keys::id(&keys::stream(&self.config.namespace,source))})
                .await
                .map_err(unavailable)?
                .ok_or_else(corrupt)?;
            if MeasuredStream::read(&metadata)?.id != stream {
                return Err(corrupt());
            }
            result.push(stream.to_owned());
        }
        Ok(result)
    }
}

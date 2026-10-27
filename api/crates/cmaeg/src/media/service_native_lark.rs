use super::{Instant, MediaService, MediaTarget, JOB_CAP};

impl MediaService {
    pub(super) async fn refresh_native(
        &self,
        target: &MediaTarget,
        index: usize,
    ) -> Result<Option<MediaTarget>, crate::media::FetchError> {
        let Some(locator @ cmaeg_core::activity::AttachmentSource::NativeLark { .. }) =
            target.attachment(index).and_then(|a| a.source.as_ref())
        else {
            return Ok(None);
        };
        let resolver = self
            .lark_resolver
            .as_ref()
            .ok_or(crate::media::FetchError::NotRetrievable)?;
        let current = crate::media::source::native_lark_authority::refresh(
            resolver,
            &crate::media::source::FetchRequest {
                channel: &target.channel,
                event: &target.event,
                activity: &target.activity,
                locator: Some(locator),
                declared_size: None,
                credential: crate::media::source::SourceCredential::ChannelKey,
                cap: 0,
                overflow: crate::media::body::Overflow::Reject,
                deadline: Instant::now() + JOB_CAP,
            },
        )
        .await?;
        Ok(Some(MediaTarget {
            channel: current,
            ..target.clone()
        }))
    }
}

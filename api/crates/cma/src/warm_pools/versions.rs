//! Optional display metadata keyed by immutable image reference.
use std::{collections::HashSet, time::Duration};

use futures_util::{stream, StreamExt};
use layer1_public::rest::admin::warm_pools::WarmPoolRuntimeVersion;
use moka::sync::Cache;

use crate::runtime_pointer::RuntimeRegistry;

pub(super) struct RuntimeVersions {
    registry: RuntimeRegistry,
    versions: Cache<String, Option<String>>,
}

impl RuntimeVersions {
    pub(super) fn new() -> Option<Self> {
        Some(Self {
            registry: RuntimeRegistry::new(None).ok()?,
            versions: Cache::builder()
                .max_capacity(256)
                .time_to_live(Duration::from_secs(300))
                .build(),
        })
    }

    /// Return whatever metadata fits the inventory read budget. Registry
    /// failure never hides a pool or affects the claim path.
    pub(super) async fn list<'a>(
        &self,
        images: impl IntoIterator<Item = &'a str>,
    ) -> Vec<WarmPoolRuntimeVersion> {
        let mut seen = HashSet::new();
        let mut found = Vec::new();
        let mut missing = Vec::new();
        for image in images.into_iter().filter(|image| seen.insert(*image)) {
            match self.versions.get(image) {
                Some(Some(version)) => found.push(WarmPoolRuntimeVersion {
                    image: image.into(),
                    version,
                }),
                Some(None) => {}
                None => missing.push(image),
            }
        }
        let requests = stream::iter(missing)
            .map(|image| async move {
                let version = self.registry.version(image).await;
                self.versions.insert(image.into(), version.clone());
                version.map(|version| WarmPoolRuntimeVersion {
                    image: image.into(),
                    version,
                })
            })
            .buffer_unordered(4);
        tokio::pin!(requests);
        let deadline = tokio::time::sleep(Duration::from_secs(2));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => break,
                result = requests.next() => match result {
                    Some(Some(entry)) => found.push(entry),
                    Some(None) => {}
                    None => break,
                },
            }
        }
        found.sort_unstable_by(|left, right| left.image.cmp(&right.image));
        found
    }
}

//! Pool images come from the process's released-default authority.
use super::{deployment_image, NativeWarmPools};
use layer3_adapter::RuntimeImage;
use std::sync::Arc;

/// One consistent view of the deployment-owned pool images.
pub(crate) struct ReleasedImages {
    pub(crate) agent: RuntimeImage,
    pub(crate) machine: Option<(RuntimeImage, u32)>,
}

/// Fixed pins for both lanes and the machine pool's target.
pub(super) fn pinned(
    agent: String,
    machine: Option<(String, u32)>,
) -> (Arc<dyn layer3_adapter::ReleasedRuntimeSource>, u32) {
    let target = machine.as_ref().map_or(0, |(_, target)| *target);
    let machine = machine.map(|(image, _)| image);
    let pinned = layer3_adapter::PinnedReleasedRuntime::new(agent, machine);
    (Arc::new(pinned), target)
}

impl NativeWarmPools {
    /// Share the process's released-default authority instead of fixed pins.
    pub(crate) fn with_released_runtime(
        mut self: Arc<Self>,
        released: Arc<dyn layer3_adapter::ReleasedRuntimeSource>,
    ) -> Arc<Self> {
        Arc::make_mut(&mut self).released = released;
        self
    }

    #[cfg(test)]
    pub(crate) fn set_released(&mut self, agent: String, machine: Option<(String, u32)>) {
        (self.released, self.machine_target) = pinned(agent, machine);
    }

    /// The default plan's identity for the image the caller already captured.
    pub(super) fn default_config_sha256(&self, image: &RuntimeImage) -> Option<String> {
        super::default_config_sha256(
            self.adapter.native_pools(),
            image,
            self.home.as_ref(),
            &self.default_capacity,
        )
    }

    pub(crate) fn released_images(&self) -> ReleasedImages {
        let released = self.released.snapshot();
        ReleasedImages {
            agent: deployment_image(released.agent),
            machine: released
                .machine
                .map(|image| (deployment_image(image), self.machine_target)),
        }
    }
}

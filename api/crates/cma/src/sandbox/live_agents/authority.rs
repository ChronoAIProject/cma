use super::*;

pub(super) fn validate_authority(
    state: &RegistryState,
    agent_id: &AgentId,
    runtime_generation: u64,
    authority_fence: u64,
) -> Result<(), LiveAgentRegistryError> {
    if runtime_generation == 0 || authority_fence == 0 {
        return Err(LiveAgentRegistryError::StaleGeneration);
    }
    if state.watermarks.get(agent_id).is_some_and(|watermark| {
        runtime_generation < watermark.runtime_generation
            || (runtime_generation == watermark.runtime_generation
                && (authority_fence < watermark.authority_fence
                    || (watermark.fenced && authority_fence == watermark.authority_fence)))
    }) {
        return Err(LiveAgentRegistryError::StaleGeneration);
    }
    Ok(())
}

pub(super) fn accept_authority(
    state: &mut RegistryState,
    agent_id: &AgentId,
    runtime_generation: u64,
    authority_fence: u64,
) {
    let watermark = state.watermarks.entry(agent_id.clone()).or_default();
    if runtime_generation > watermark.runtime_generation {
        watermark.runtime_generation = runtime_generation;
        watermark.authority_fence = authority_fence;
        watermark.fenced = false;
    } else if runtime_generation == watermark.runtime_generation
        && authority_fence > watermark.authority_fence
    {
        watermark.authority_fence = authority_fence;
        watermark.fenced = false;
    }
}

pub(super) fn fence_authority(
    state: &mut RegistryState,
    agent_id: &AgentId,
    runtime_generation: u64,
    authority_fence: u64,
) -> bool {
    let watermark = state.watermarks.entry(agent_id.clone()).or_default();
    if runtime_generation > watermark.runtime_generation
        || (runtime_generation == watermark.runtime_generation
            && authority_fence >= watermark.authority_fence)
    {
        watermark.runtime_generation = runtime_generation;
        watermark.authority_fence = authority_fence;
        watermark.fenced = true;
        true
    } else {
        false
    }
}

use super::*;

pub(crate) struct FundingRuntime<'a> {
    sandbox: &'a Sandbox,
    generation: u64,
}

impl<'a> FundingRuntime<'a> {
    pub(super) fn for_state(
        state: &SandboxBillingState,
        sandbox: &'a Sandbox,
    ) -> Result<Option<Self>, FundingError> {
        require_identity(state, sandbox)?;
        let generation = generation(sandbox);
        if generation != state.runtime_generation
            || sandbox.runtime.as_ref().is_some_and(|binding| {
                binding.provision_generation.map_or_else(
                    || sandbox.lifecycle.runtime_provision.is_some(),
                    |bound| bound != generation,
                )
            })
        {
            return Ok(None);
        }
        Ok(Some(Self {
            sandbox,
            generation,
        }))
    }

    pub(super) fn sandbox(&self) -> &Sandbox {
        self.sandbox
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn confirmed(
        self,
        receipt: &FundingWindow,
    ) -> Result<FundedRuntime<'a>, FundingError> {
        if receipt.sandbox_id != self.sandbox.id.as_str()
            || receipt.owner_id != self.sandbox.owner.as_str()
            || receipt.runtime_generation != self.generation
        {
            return Err(FundingError::Invalid(
                "funding receipt targets another runtime".into(),
            ));
        }
        Ok(FundedRuntime {
            target: self,
            until: receipt.covered_until,
        })
    }
}

pub(crate) struct FundedRuntime<'a> {
    target: FundingRuntime<'a>,
    until: u64,
}

impl FundedRuntime<'_> {
    pub(super) fn target(&self) -> &FundingRuntime<'_> {
        &self.target
    }

    pub(super) fn until(&self) -> u64 {
        self.until
    }
}

pub(super) fn completion_applies(
    state: &SandboxBillingState,
    sandbox: &Sandbox,
) -> Result<bool, FundingError> {
    require_identity(state, sandbox)?;
    let binding = sandbox
        .runtime
        .as_ref()
        .or(sandbox.lifecycle.suspended_runtime.as_deref());
    let completed = match binding.and_then(|binding| binding.provision_generation) {
        Some(generation) => generation,
        None if binding.is_some() && sandbox.lifecycle.runtime_provision.is_some() => {
            return Err(FundingError::Conflict);
        }
        None => generation(sandbox),
    };
    match completed.cmp(&state.runtime_generation) {
        std::cmp::Ordering::Less => Ok(false),
        std::cmp::Ordering::Equal => Ok(true),
        std::cmp::Ordering::Greater => Err(FundingError::Conflict),
    }
}

pub(super) fn require_identity(
    state: &SandboxBillingState,
    sandbox: &Sandbox,
) -> Result<(), FundingError> {
    if state.sandbox_id != sandbox.id.as_str() || state.owner_id != sandbox.owner.as_str() {
        return Err(FundingError::Invalid(
            "funding owner or Sandbox changed".into(),
        ));
    }
    Ok(())
}

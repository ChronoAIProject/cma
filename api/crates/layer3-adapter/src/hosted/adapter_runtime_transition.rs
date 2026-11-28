use super::*;

impl HostedAdapter {
    pub(in crate::hosted) async fn resume_exact(
        &self,
        request: &RuntimeAdoptionRequest,
        successor: &crate::RuntimeProvisionRequest,
    ) -> crate::Result<RuntimeHandle> {
        self.transition_exact(request, successor, false, None).await
    }

    pub(in crate::hosted) async fn upgrade_exact(
        &self,
        request: &RuntimeAdoptionRequest,
        successor: &crate::RuntimeProvisionRequest,
    ) -> crate::Result<RuntimeHandle> {
        self.transition_exact(request, successor, true, None).await
    }

    pub(in crate::hosted) async fn upgrade_retained_exact(
        &self,
        request: &RuntimeAdoptionRequest,
        successor: &crate::RuntimeProvisionRequest,
        admission: &crate::RetainedStartupPolicyUpgrade<'_>,
    ) -> crate::Result<RuntimeHandle> {
        self.transition_exact(request, successor, true, Some(admission))
            .await
    }

    async fn transition_exact(
        &self,
        request: &RuntimeAdoptionRequest,
        successor: &crate::RuntimeProvisionRequest,
        upgrade: bool,
        retained: Option<&crate::RetainedStartupPolicyUpgrade<'_>>,
    ) -> crate::Result<RuntimeHandle> {
        let mut allowed = request.spec.clone();
        if let Some(admission) = retained {
            if successor.generation != admission.successor_generation
                || successor.spec.runtime_image.as_deref() != Some(admission.target_image)
            {
                return Err(AdapterError::Rejected(
                    "retained runtime successor differs from its admission".into(),
                ));
            }
            allowed.retained_journal_recovery = Some(admission.journal_recovery);
        } else {
            allowed.reaches.clone_from(&successor.spec.reaches);
        }
        if upgrade {
            if successor.spec.runtime_image.is_none()
                || request.expected_image.is_none()
                || !request.spec.home_volume.as_ref().is_some_and(|home| {
                    home.request.policy.is_native_vct() && home.receipt.native_authority.is_some()
                })
            {
                return Err(AdapterError::Rejected(
                    "runtime upgrade requires an explicit image and original native VCT home"
                        .into(),
                ));
            }
            allowed
                .runtime_image
                .clone_from(&successor.spec.runtime_image);
        }
        if successor.generation <= request.generation || successor.spec != allowed {
            return Err(AdapterError::Rejected(
                "runtime transition must advance generation and retain its admitted specification"
                    .into(),
            ));
        }
        let previous = self.retained_request(request, request.generation)?;
        // Preserve the persisted image even when the deployment default changed.
        let mut resumed = request.clone();
        resumed.spec = successor.spec.clone();
        resumed.generation = successor.generation;
        if upgrade {
            resumed
                .expected_image
                .clone_from(&successor.spec.runtime_image);
        }
        let mut next = self.retained_request(&resumed, successor.generation)?;
        if let Some(admission) = retained {
            let frozen_successor = previous.retained_startup_policy_successor(
                admission.target_image,
                admission.successor_generation,
                admission.journal_recovery,
            )?;
            if next != frozen_successor {
                return Err(AdapterError::Rejected(
                    "rendered retained successor changes its admitted specification".into(),
                ));
            }
        }
        next.funded_until = successor.funded_until;
        self.backend
            .validate_funding(&next, Some(&request.handle))
            .await?;
        let previous_identity = provider_request_identity(&previous)?;
        let next_identity = provider_request_identity(&next)?;
        self.verify_spec_home(&request.spec).await?;
        self.verify_adoption_home_route(request)?;
        {
            let mut states = self.states.lock().await;
            if let Some(state) = states.get(&request.handle) {
                // Suspension on one replica invalidates only that replica's cache.
                // The provider still checks the exact predecessor or successor.
                let cached_before_predecessor = !upgrade
                    && state.provision.sandbox_id == previous_identity.sandbox_id
                    && state.provision.generation < previous_identity.generation
                    && state.provision.image_sha256 == previous_identity.image_sha256
                    && state.provision.runtime_contract == previous_identity.runtime_contract
                    && state.home_volume == request.spec.home_volume
                    && state.home_directory == request.spec.home_directory
                    && state.own_credential == request.spec.own_credential;
                if state.provision != previous_identity
                    && state.provision != next_identity
                    && !cached_before_predecessor
                {
                    return Err(AdapterError::Rejected(
                        "resume conflicts with another runtime generation".into(),
                    ));
                }
            }
            states.remove(&request.handle);
        }
        let home = request.spec.home_volume.as_deref().ok_or_else(|| {
            AdapterError::Rejected("runtime transition requires its persisted home".into())
        })?;
        if let Some(admission) = retained {
            crate::hosted::retained_startup_policy::require(self.backend.as_ref())?
                .upgrade_retained_policy_with_home(
                    &request.handle,
                    &previous,
                    &next,
                    home,
                    admission,
                )
                .await?;
        } else if upgrade {
            self.backend
                .upgrade_with_home(&request.handle, &previous, &next, home)
                .await?;
        } else {
            self.backend
                .resume_with_home(&request.handle, &previous, &next, home)
                .await?;
        }
        self.verify_spec_home(&request.spec).await?;
        self.attach_state(request.handle.clone(), &successor.spec, next_identity)
            .await
    }
}

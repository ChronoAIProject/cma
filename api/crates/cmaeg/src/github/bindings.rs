use super::*;

// The requested/approved scopes, effective IDs and exclusions must fit the
// existing sealed-record bound even for 20-digit IDs and full audit strings.
const MAX_REPOSITORIES: usize = 256;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingOperation {
    fingerprint: String,
    response: Binding,
}

impl Gateway {
    pub(super) async fn bind(
        &self,
        owner: &str,
        key: &str,
        request: BindRequest,
    ) -> Result<(Binding, bool), Error> {
        retry::run(|| self.bind_once(owner, key, request.clone())).await
    }

    async fn bind_once(
        &self,
        owner: &str,
        key: &str,
        request: BindRequest,
    ) -> Result<(Binding, bool), Error> {
        let app = self.read_app(owner, &request.app_ref).await?;
        if app.status == "retired" {
            return Err(Error::Retired);
        }
        if app.status != "verified" || request.proof_ref.is_some() {
            return Err(Error::Forbidden);
        }
        integrations::validate_key(key)?;
        let fingerprint = identity("binding-input", &request);
        let operation_id = identity("binding-operation", &(owner, "create", key));
        match self
            .binding_operation(owner, &operation_id, &fingerprint)
            .await
        {
            Ok(binding) => return Ok((binding, false)),
            Err(Error::NotFound) => (),
            Err(error) => return Err(error),
        }
        let account_id = request
            .account_id
            .filter(|id| *id > 0)
            .ok_or(Error::Invalid)?;
        let repository_ids = request.repository_ids.ok_or(Error::Invalid)?;
        if request.installation_id == 0
            || repository_ids.is_empty()
            || repository_ids.len() > MAX_REPOSITORIES
            || repository_ids.contains(&0)
            || repository_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != repository_ids.len()
        {
            return Err(Error::Invalid);
        }
        let binding_id = identity("binding", &(&app.app_ref, request.installation_id));
        match self
            .installation_binding(owner, &app, request.installation_id)
            .await
        {
            Ok(binding) => {
                if binding.account_id != account_id
                    || binding.requested_repository_ids != repository_ids
                {
                    return Err(Error::ManagementConflict);
                }
                let operation = BindingOperation {
                    fingerprint,
                    response: binding.clone(),
                };
                self.store
                    .create(&operation_id, owner, "binding_operation", &operation)
                    .await?;
                return Ok((binding, false));
            }
            Err(Error::NotFound) => (),
            Err(error) => return Err(error),
        }
        let binding = Binding {
            binding_id,
            app_ref: app.app_ref.clone(),
            app_id: app.app_id,
            installation_id: request.installation_id,
            account_id,
            status: "pending_verification".into(),
            authority_revision: 0,
            verified_until: 0,
            verified_by: String::new(),
            verification_ref: String::new(),
            repository_ids: Vec::new(),
            version: 1,
            requested_repository_ids: repository_ids,
            approved_repository_ids: Vec::new(),
            excluded_repository_ids: Vec::new(),
            approval: None,
        };
        let operation = BindingOperation {
            fingerprint: fingerprint.clone(),
            response: binding.clone(),
        };
        let records = vec![
            store::NewRecord::new(&binding.binding_id, "binding", &binding)?,
            store::NewRecord::new(&operation_id, "binding_operation", &operation)?,
        ];
        if let Err(failure) = self
            .store
            .create_many_guarded(owner, records, Some(&app))
            .await
        {
            return self
                .binding_operation(owner, &operation_id, &fingerprint)
                .await
                .map(|binding| (binding, false))
                .map_err(|error| {
                    if matches!(error, Error::NotFound) {
                        failure
                    } else {
                        error
                    }
                });
        }
        Ok((binding, true))
    }

    async fn binding_operation(
        &self,
        owner: &str,
        id: &str,
        fingerprint: &str,
    ) -> Result<Binding, Error> {
        let (stored_owner, operation) = self
            .store
            .read::<BindingOperation>(id, "binding_operation")
            .await?;
        if stored_owner != owner {
            return Err(Error::NotFound);
        }
        if operation.fingerprint != fingerprint {
            return Err(Error::ManagementConflict);
        }
        Ok(operation.response)
    }

    pub(super) async fn verify_binding(
        &self,
        owner: &str,
        id: &str,
        verifier: &str,
        key: &str,
        request: VerifyBindingRequest,
    ) -> Result<Binding, Error> {
        retry::run(|| self.verify_binding_once(owner, id, verifier, key, request.clone())).await
    }

    async fn verify_binding_once(
        &self,
        owner: &str,
        id: &str,
        verifier: &str,
        key: &str,
        request: VerifyBindingRequest,
    ) -> Result<Binding, Error> {
        integrations::validate_key(key)?;
        if request.expected_version == 0
            || request.evidence_ref.trim().is_empty()
            || request.evidence_ref.len() > 512
            || request.evidence_ref.chars().any(char::is_control)
            || request.repository_ids.is_empty()
            || request.repository_ids.len() > MAX_REPOSITORIES
            || request.repository_ids.contains(&0)
            || request
                .repository_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != request.repository_ids.len()
        {
            return Err(Error::Invalid);
        }
        let mut binding = self.binding(owner, id).await?;
        let app = self.read_app(owner, &binding.app_ref).await?;
        if app.status == "retired" {
            return Err(Error::Retired);
        }
        if app.status != "verified" {
            return Err(Error::Forbidden);
        }
        let fingerprint = identity("binding-verification-input", &request);
        let operation_id = identity("binding-operation", &(owner, id, "verification", key));
        match self
            .binding_operation(owner, &operation_id, &fingerprint)
            .await
        {
            Ok(binding) => return Ok(binding),
            Err(Error::NotFound) => (),
            Err(error) => return Err(error),
        }
        if binding.version != request.expected_version {
            return Err(Error::VersionConflict(binding.version));
        }
        if binding.status == "uninstalled" {
            return Err(Error::Forbidden);
        }
        let eligibility_changed = binding.status != "active"
            || binding
                .repository_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                != request
                    .repository_ids
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>();
        binding.version = binding.version.checked_add(1).ok_or(Error::Unavailable)?;
        if eligibility_changed {
            binding.authority_revision = binding
                .authority_revision
                .checked_add(1)
                .ok_or(Error::Unavailable)?;
        }
        binding.status = "active".into();
        binding.approved_repository_ids = request.repository_ids.clone();
        binding.repository_ids = request.repository_ids.clone();
        binding.excluded_repository_ids.retain(|id| {
            binding.requested_repository_ids.contains(id) && !request.repository_ids.contains(id)
        });
        binding.verified_by = verifier.into();
        binding.verification_ref = request.evidence_ref.clone();
        let approval = BindingApproval {
            verified_by: verifier.into(),
            evidence_ref: request.evidence_ref,
            approved_at: now(),
            authority_revision: binding.authority_revision,
            repository_ids: request.repository_ids,
        };
        binding.approval = Some(approval.clone());
        let records = vec![
            store::NewRecord::new(
                &operation_id,
                "binding_operation",
                &BindingOperation {
                    fingerprint: fingerprint.clone(),
                    response: binding.clone(),
                },
            )?,
            store::NewRecord::new(
                &identity("binding-approval", &(id, binding.version)),
                "binding_approval",
                &approval,
            )?,
        ];
        if let Err(error) = self
            .store
            .update_binding(owner, request.expected_version, &binding, &app, records)
            .await
        {
            return match self
                .binding_operation(owner, &operation_id, &fingerprint)
                .await
            {
                Err(Error::NotFound) => Err(error),
                result => result,
            };
        }
        Ok(binding)
    }
}

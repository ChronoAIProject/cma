use super::*;

impl Gateway {
    pub(super) async fn bind(
        &self,
        owner: &str,
        request: BindRequest,
    ) -> Result<(Binding, bool), Error> {
        let app = self.config.app(&request.app_ref).ok_or(Error::Forbidden)?;
        let binding_id = identity("binding", &(app.app_id, request.installation_id));
        match self.store.read::<Binding>(&binding_id, "binding").await {
            Ok((stored_owner, binding)) if stored_owner == owner => return Ok((binding, false)),
            Ok(_) => return Err(Error::Forbidden),
            Err(Error::NotFound) => (),
            Err(error) => return Err(error),
        }
        let proof = app
            .attestations
            .iter()
            .find(|proof| {
                proof.owner == owner
                    && proof.proof_ref == request.proof_ref
                    && proof.installation_id == request.installation_id
                    && proof.verified_until > now()
            })
            .ok_or(Error::Forbidden)?;
        let binding = Binding {
            binding_id,
            app_ref: app.app_ref.clone(),
            app_id: app.app_id,
            installation_id: proof.installation_id,
            account_id: proof.account_id,
            status: "active".into(),
            authority_revision: 1,
            verified_until: proof.verified_until,
            verified_by: app.operator_owner.clone(),
            verification_ref: format!("{:x}", Sha256::digest(proof.proof_ref.as_bytes())),
            repository_ids: proof.repository_ids.clone(),
        };
        if self
            .store
            .create(&binding.binding_id, owner, "binding", &binding)
            .await
            .is_err()
        {
            let (stored_owner, committed) = self
                .store
                .read::<Binding>(&binding.binding_id, "binding")
                .await
                .map_err(|_| Error::Unavailable)?;
            if stored_owner != owner {
                return Err(Error::Forbidden);
            }
            return Ok((committed, false));
        }
        Ok((binding, true))
    }

    pub(super) async fn installation(
        &self,
        owner: &str,
        mut binding: Binding,
        incoming: LifecycleDelivery<'_>,
    ) -> Result<(Receipt, bool), Error> {
        let LifecycleDelivery {
            receipt_id,
            delivery_id,
            fingerprint,
            event,
            repository_id,
            payload,
            body,
        } = incoming;
        if event == "repository" {
            if payload.get("issue").is_some()
                || payload.get("pull_request").is_some()
                || payload.get("installation").is_none_or(|installation| {
                    installation
                        .get("app_id")
                        .and_then(serde_json::Value::as_u64)
                        .is_some_and(|app_id| app_id != binding.app_id)
                })
                || repository_id.is_none_or(|id| id == 0)
                || payload
                    .pointer("/repository/owner/id")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|id| id == 0)
            {
                return Err(Error::Invalid);
            }
        } else if payload
            .pointer("/installation/app_id")
            .and_then(serde_json::Value::as_u64)
            != Some(binding.app_id)
            || payload
                .pointer("/installation/account/id")
                .and_then(serde_json::Value::as_u64)
                != Some(binding.account_id)
            || payload.get("issue").is_some()
            || payload.get("pull_request").is_some()
        {
            return Err(Error::Invalid);
        }
        let action = payload
            .get("action")
            .and_then(serde_json::Value::as_str)
            .ok_or(Error::Invalid)?;
        if event == "repository" {
            if !matches!(action, "created" | "renamed" | "transferred" | "deleted") {
                return Err(Error::Invalid);
            }
            let repository_id = repository_id.ok_or(Error::Invalid)?;
            let repository_owner = payload
                .pointer("/repository/owner/id")
                .and_then(serde_json::Value::as_u64)
                .ok_or(Error::Invalid)?;
            if action == "transferred"
                || action == "deleted"
                || repository_owner != binding.account_id
            {
                binding.repository_ids.retain(|id| *id != repository_id);
            }
        } else if event == "installation_repositories" {
            if !matches!(action, "added" | "removed")
                || !payload
                    .get("repositories_added")
                    .is_some_and(serde_json::Value::is_array)
                || !payload
                    .get("repositories_removed")
                    .is_some_and(serde_json::Value::is_array)
            {
                return Err(Error::Invalid);
            }
            if action == "removed" {
                let removed = payload
                    .get("repositories_removed")
                    .and_then(serde_json::Value::as_array)
                    .ok_or(Error::Invalid)?;
                let ids = removed
                    .iter()
                    .map(|repository| {
                        repository
                            .get("id")
                            .and_then(serde_json::Value::as_u64)
                            .filter(|id| *id > 0)
                            .ok_or(Error::Invalid)
                    })
                    .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
                binding.repository_ids.retain(|id| !ids.contains(id));
            }
        } else {
            if !matches!(
                action,
                "created" | "deleted" | "suspend" | "unsuspend" | "new_permissions_accepted"
            ) || payload.get("repositories_added").is_some()
                || payload.get("repositories_removed").is_some()
            {
                return Err(Error::Invalid);
            }
            binding.status = match action {
                "suspend" if binding.status != "uninstalled" => "suspended",
                "deleted" => "uninstalled",
                "unsuspend" | "created" | "new_permissions_accepted"
                    if binding.status != "uninstalled" =>
                {
                    "pending_verification"
                }
                _ => &binding.status,
            }
            .into();
        }
        binding.authority_revision = binding
            .authority_revision
            .checked_add(1)
            .ok_or(Error::Unavailable)?;
        let receipt = Receipt {
            receipt_id,
            app_ref: binding.app_ref.clone(),
            installation_id: Some(binding.installation_id),
            delivery_id: delivery_id.into(),
            repository_id,
            event: event.into(),
            action: Some(action.into()),
            status: "ignored".into(),
            received_at: now(),
            raw_body_expires_at: now() + 7 * 24 * 60 * 60,
            body_sha256: fingerprint.into(),
            task_ids: Vec::new(),
            authority_revision: binding.authority_revision,
        };
        let delivery = Delivery {
            receipt: receipt.clone(),
            raw_body_records: self.store.source(owner, &receipt, body).await?,
        };
        if self
            .store
            .commit_with_authority(owner, &binding, binding.authority_revision - 1, &delivery)
            .await
            .is_err()
        {
            let (_, committed) = self
                .store
                .read::<Delivery>(&receipt.receipt_id, "delivery")
                .await
                .map_err(|_| Error::Unavailable)?;
            return duplicate(committed, event, fingerprint);
        }
        Ok((receipt, true))
    }
}

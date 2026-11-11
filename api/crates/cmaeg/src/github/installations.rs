use super::*;

impl Gateway {
    pub(super) async fn installation(
        &self,
        owner: &str,
        mut binding: Binding,
        incoming: LifecycleDelivery<'_>,
    ) -> Result<(Receipt, bool), Error> {
        let previous = binding.clone();
        let mut repository_tightening = false;
        let LifecycleDelivery {
            integration,
            received_at,
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
                repository_tightening = true;
                binding.exclude_repository(repository_id);
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
                repository_tightening = !ids.is_empty();
                for id in ids {
                    binding.exclude_repository(id);
                }
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
                _ => &binding.status,
            }
            .into();
        }
        let eligibility_changed = binding.status != previous.status
            || (binding.status == "active" && binding.repository_ids != previous.repository_ids);
        if eligibility_changed {
            binding.authority_revision = binding
                .authority_revision
                .checked_add(1)
                .ok_or(Error::Unavailable)?;
        }
        if repository_tightening || binding != previous {
            binding.version = binding.version.checked_add(1).ok_or(Error::Unavailable)?;
        }
        let receipt = Receipt {
            receipt_id,
            app_ref: binding.app_ref.clone(),
            installation_id: Some(binding.installation_id),
            delivery_id: delivery_id.into(),
            repository_id,
            event: event.into(),
            action: Some(action.into()),
            status: "ignored".into(),
            reason: None,
            received_at,
            raw_body_expires_at: received_at + 7 * 86400,
            retained_until: Some(received_at + 7 * 86400),
            body_sha256: fingerprint.into(),
            task_ids: Vec::new(),
            authority_revision: binding.authority_revision,
            dispatch: None,
        };
        if binding.version == previous.version {
            return self.persist(owner, integration, receipt, body, None).await;
        }
        let delivery = Delivery {
            receipt: receipt.clone(),
            raw_body_records: self.store.source(owner, &receipt, body).await?,
            dispatch: None,
        };
        if let Err(error) = self
            .store
            .commit_with_authority(owner, &binding, integration, previous.version, &delivery)
            .await
        {
            return match self
                .store
                .read::<Delivery>(&receipt.receipt_id, "delivery")
                .await
            {
                Ok((_, committed)) => duplicate(committed, event, fingerprint),
                Err(Error::NotFound) if matches!(error, Error::Retry) => Err(Error::Retry),
                _ => Err(Error::Unavailable),
            };
        }
        Ok((receipt, true))
    }
}

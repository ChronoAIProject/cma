use super::*;

impl Gateway {
    pub(super) async fn receive(
        &self,
        app_ref: &str,
        delivery_id: &str,
        event: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<(Receipt, bool), Error> {
        let (integration_owner, app, webhook_secrets) = self.webhook_credentials(app_ref).await?;
        let hex = signature
            .strip_prefix("sha256=")
            .filter(|value| value.len() == 64)
            .ok_or(Error::Signature)?;
        let tag: Vec<u8> = hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let pair = std::str::from_utf8(pair).map_err(|_| Error::Signature)?;
                u8::from_str_radix(pair, 16).map_err(|_| Error::Signature)
            })
            .collect::<Result<_, _>>()?;
        if !webhook_secrets.iter().any(|secret| {
            ring::hmac::verify(
                &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_str().as_bytes()),
                body,
                &tag,
            )
            .is_ok()
        }) {
            return Err(Error::Signature);
        }
        let receipt_id = identity("delivery", &(app_ref, delivery_id));
        let fingerprint = format!("{:x}", Sha256::digest(body));
        match self.store.read::<Delivery>(&receipt_id, "delivery").await {
            Ok((_, delivery)) => return duplicate(delivery, event, &fingerprint),
            Err(Error::NotFound) => (),
            Err(error) => return Err(error),
        }
        let payload: serde_json::Value =
            serde_json::from_slice(body).map_err(|_| Error::Invalid)?;
        if event == "ping" {
            if !payload.get("zen").is_some_and(serde_json::Value::is_string)
                || payload
                    .get("hook_id")
                    .and_then(serde_json::Value::as_u64)
                    .is_none()
                || !payload
                    .get("hook")
                    .is_some_and(serde_json::Value::is_object)
                || payload.get("action").is_some()
            {
                return Err(Error::Invalid);
            }
            let receipt = Receipt {
                receipt_id,
                app_ref: app_ref.into(),
                installation_id: None,
                delivery_id: delivery_id.into(),
                repository_id: None,
                event: event.into(),
                action: None,
                status: "ignored".into(),
                received_at: now(),
                raw_body_expires_at: now() + 7 * 24 * 60 * 60,
                body_sha256: fingerprint,
                task_ids: Vec::new(),
                authority_revision: 0,
            };
            return self.persist(&integration_owner, receipt, body, None).await;
        }
        let installation_id = payload
            .pointer("/installation/id")
            .and_then(serde_json::Value::as_u64)
            .ok_or(Error::Invalid)?;
        let binding_id = identity("binding", &(app.app_id, installation_id));
        let (owner, binding): (String, Binding) = self.store.read(&binding_id, "binding").await?;
        if binding.app_ref != app_ref {
            return Err(Error::NotFound);
        }
        if matches!(
            event,
            "installation" | "installation_repositories" | "repository"
        ) {
            return self
                .installation(
                    &owner,
                    binding,
                    LifecycleDelivery {
                        receipt_id,
                        delivery_id,
                        fingerprint: &fingerprint,
                        event,
                        repository_id: if event == "repository" {
                            payload
                                .pointer("/repository/id")
                                .and_then(serde_json::Value::as_u64)
                        } else {
                            None
                        },
                        payload: &payload,
                        body,
                    },
                )
                .await;
        }
        if binding.status != "active" || binding.verified_until <= now() {
            return Err(Error::Forbidden);
        }
        let repository_id = payload
            .pointer("/repository/id")
            .and_then(serde_json::Value::as_u64)
            .ok_or(Error::Invalid)?;
        if !binding.repository_ids.contains(&repository_id)
            || payload
                .pointer("/repository/owner/id")
                .and_then(serde_json::Value::as_u64)
                != Some(binding.account_id)
        {
            return Err(Error::Forbidden);
        }
        let action = payload.get("action").and_then(serde_json::Value::as_str);
        if (payload.get("issue").is_some() && payload.get("comment").is_none() && event != "issues")
            || (payload.get("pull_request").is_some()
                && payload.get("review").is_none()
                && payload.get("comment").is_none()
                && payload.get("thread").is_none()
                && event != "pull_request")
        {
            return Err(Error::Invalid);
        }
        let mut supported = false;
        if matches!(event, "issues" | "pull_request") {
            let action = action
                .filter(|action| config::valid_ref(action))
                .ok_or(Error::Invalid)?;
            if payload.get("comment").is_some()
                || payload.get("review").is_some()
                || payload.get("thread").is_some()
            {
                return Err(Error::Invalid);
            }
            let object = match event {
                "issues" if payload.get("pull_request").is_none() => payload.get("issue"),
                "pull_request" if payload.get("issue").is_none() => payload.get("pull_request"),
                _ => None,
            }
            .ok_or(Error::Invalid)?;
            if object
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .is_none_or(|id| id == 0)
                || object
                    .get("number")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|number| number == 0)
            {
                return Err(Error::Invalid);
            }
            supported = matches!(
                action,
                "opened"
                    | "edited"
                    | "closed"
                    | "reopened"
                    | "labeled"
                    | "unlabeled"
                    | "assigned"
                    | "unassigned"
                    | "milestoned"
                    | "demilestoned"
                    | "locked"
                    | "unlocked"
            ) || (event == "issues"
                && matches!(
                    action,
                    "deleted" | "transferred" | "pinned" | "unpinned" | "typed" | "untyped"
                ))
                || (event == "pull_request"
                    && matches!(
                        action,
                        "synchronize"
                            | "ready_for_review"
                            | "converted_to_draft"
                            | "review_requested"
                            | "review_request_removed"
                            | "auto_merge_enabled"
                            | "auto_merge_disabled"
                            | "enqueued"
                            | "dequeued"
                    ));
        }
        let (status, task_ids) = if supported {
            match self
                .dispatch(DispatchInput {
                    owner: &owner,
                    app_ref,
                    binding: &binding,
                    receipt_id: &receipt_id,
                    delivery_id,
                    event,
                    action,
                    payload: &payload,
                })
                .await?
            {
                None => ("accepted", Vec::new()),
                Some(task_ids) if task_ids.is_empty() => ("ignored", task_ids),
                Some(task_ids) => ("accepted", task_ids),
            }
        } else {
            ("ignored", Vec::new())
        };
        let receipt = Receipt {
            receipt_id,
            app_ref: app_ref.into(),
            installation_id: Some(installation_id),
            delivery_id: delivery_id.into(),
            repository_id: Some(repository_id),
            event: event.into(),
            action: action.map(str::to_owned),
            status: status.into(),
            received_at: now(),
            raw_body_expires_at: now() + 7 * 24 * 60 * 60,
            body_sha256: fingerprint.clone(),
            task_ids,
            authority_revision: binding.authority_revision,
        };
        self.persist(&owner, receipt, body, Some(&binding)).await
    }

    pub(super) async fn persist(
        &self,
        owner: &str,
        receipt: Receipt,
        body: &[u8],
        authority: Option<&Binding>,
    ) -> Result<(Receipt, bool), Error> {
        let delivery = Delivery {
            receipt: receipt.clone(),
            raw_body_records: self.store.source(owner, &receipt, body).await?,
        };
        let committed = match authority {
            Some(binding) => {
                self.store
                    .commit_with_authority(owner, binding, binding.authority_revision, &delivery)
                    .await
            }
            None => {
                self.store
                    .create(&receipt.receipt_id, owner, "delivery", &delivery)
                    .await
            }
        };
        if let Err(error) = committed {
            return match self
                .store
                .read::<Delivery>(&receipt.receipt_id, "delivery")
                .await
            {
                Ok((_, committed)) => duplicate(committed, &receipt.event, &receipt.body_sha256),
                Err(Error::NotFound) if matches!(error, Error::Forbidden) => Err(Error::Forbidden),
                Err(_) => Err(Error::Unavailable),
            };
        }
        Ok((receipt, true))
    }

    pub(super) async fn receipt(&self, owner: &str, id: &str) -> Result<Receipt, Error> {
        let (stored_owner, delivery): (String, Delivery) = self.store.read(id, "delivery").await?;
        if stored_owner != owner {
            return Err(Error::NotFound);
        }
        Ok(delivery.receipt)
    }
}

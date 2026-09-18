use super::*;

impl NyxBridgeProvider {
    /// Prove possession of the exact selected key with a read that NyxID fences
    /// against the authenticated API-key ID. Returned messages are ignored.
    pub async fn verify_secret(
        &self,
        owner: &ResourceOwner,
        secret: &SecretValue,
        connector: &BridgeConnector,
        human_subject: &SubjectId,
        human_credential: &str,
    ) -> Result<VerifiedProviderBinding, BridgeError> {
        self.verify_secret_with_authority(
            owner,
            secret,
            connector,
            human_subject,
            human_credential,
            None,
        )
        .await
    }

    /// Retained custody already pins the complete key authority from import.
    /// Reuse that baseline, while still proving possession and reading all
    /// current provider authority after the probe on every resolution.
    pub(in crate::bridge) async fn verify_secret_with_authority(
        &self,
        owner: &ResourceOwner,
        secret: &SecretValue,
        connector: &BridgeConnector,
        human_subject: &SubjectId,
        human_credential: &str,
        retained: Option<(&KeyAuthority, i64)>,
    ) -> Result<VerifiedProviderBinding, BridgeError> {
        if !raw_agent_key(secret.expose()) || connector.route_ids.is_empty() {
            return Err(binding_mismatch());
        }
        tokio::time::timeout(DEADLINE, async {
            let start = tokio::time::Instant::now();
            self.verify_human(human_subject, human_credential).await?;
            let binding = if retained.is_none() {
                Some(
                    self.read_binding(
                        owner,
                        human_subject,
                        &SecretValue::new(human_credential.to_owned()),
                        connector,
                    )
                    .await
                    .inspect_err(|error| binding_failure("binding_before_probe", error))?,
                )
            } else {
                None
            };
            let (authority, version) = retained
                .or_else(|| {
                    binding
                        .as_ref()
                        .map(|b| (&b.key_authority, b.key_state_version))
                })
                .ok_or_else(binding_mismatch)?;
            if version <= 0 || authority.id != connector.key_id {
                return Err(binding_mismatch());
            }
            let mut endpoint = self.base_url.clone();
            endpoint.set_path("/api/v1/channel-relay/messages");
            endpoint
                .path_segments_mut()
                .map_err(|_| binding_mismatch())?
                .push(&connector.route_ids[0]);
            endpoint
                .query_pairs_mut()
                .append_pair("page", "1")
                .append_pair("per_page", "1");
            let probe: BindingProbe = self
                .fetch(endpoint, secret)
                .await
                .inspect_err(|error| binding_failure("key_possession_probe", error))?;
            if probe.page != 1
                || probe.per_page != 1
                || probe.messages.len() > 1
                || probe.total < probe.messages.len() as u64
            {
                return Err(binding_mismatch());
            }
            // Possession reads update NyxID's last_used_at and row version.
            // Recheck all current authority after that IO, within this same
            // timeout; a counter increase alone cannot identify its cause.
            let mut current = self
                .read_binding(
                    owner,
                    human_subject,
                    &SecretValue::new(human_credential.to_owned()),
                    connector,
                )
                .await
                .inspect_err(|error| binding_failure("binding_after_probe", error))?;
            if current.key_state_version < version || &current.key_authority != authority {
                binding_failure("binding_changed_during_probe", &binding_mismatch());
                return Err(binding_mismatch());
            }
            current.deadline = current
                .deadline
                .min(
                    binding
                        .as_ref()
                        .map_or(start + Duration::from_secs(60), |b| b.deadline),
                )
                .min(start + Duration::from_secs(60));
            Ok(current)
        })
        .await
        .map_err(|_| BridgeError::AuthorityUnavailable)?
    }
}

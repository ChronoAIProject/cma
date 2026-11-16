use super::*;
use axum::http::{HeaderMap, HeaderValue, Method, Uri};

#[async_trait::async_trait]
impl AgentCommandOwnerRouting for AdmissionRouter {
    async fn route(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        mutation: OwnerCommandMutation,
    ) -> Result<OwnerCommandReply, CommandAdmissionError> {
        let relay = self
            .relay
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(unavailable)?;
        let _permit = relay
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| unavailable())?;
        if !relay.coordinator.is_ready() {
            return Err(unavailable());
        }
        let id = mutation.agent_id().clone();
        let identity = self
            .auth
            .capture_owner_admission(context, credential)
            .await?;
        let call = Call {
            identity,
            credential: credential.to_owned(),
            origin: Origin::current().map_err(|_| unavailable())?,
            source: source.into(),
            intent: mutation.clone().into(),
        };
        let fresh = self.context(&call).await?;
        self.authorize(&fresh, credential, &mutation).await?;
        let destination = relay
            .control_destination(&id)
            .await
            .map_err(|_| unavailable())?;
        if destination.member.instance_id == relay.coordinator.instance_id {
            let service = self.service()?;
            let result = tokio::time::timeout(
                CALL_BOUND,
                destination
                    .authority
                    .run(service.execute_owner_mutation(&fresh, credential, source, mutation)),
            )
            .await
            .map_err(|_| unavailable())?
            .map_err(|_| unavailable())?;
            self.context(&call).await?;
            return result;
        }
        let body = serde_json::to_vec(&call).map_err(|_| unavailable())?;
        if body.len() > crate::REQUEST_BODY_LIMIT_BYTES {
            return Err(unavailable());
        }
        let request_sha256 = digest(&body);
        let uri: Uri = format!("{PREFIX}/control/{id}/admission/{}", self.name)
            .parse()
            .map_err(|_| unavailable())?;
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        // Production normalizes this signed header before verifying the peer proof.
        // Send its final canonical value so both sides sign the same header set.
        headers.insert(
            "x-request-id",
            uuid::Uuid::new_v4()
                .to_string()
                .parse()
                .map_err(|_| unavailable())?,
        );
        let proof = relay
            .authentication
            .sign(
                &relay.coordinator.instance_id,
                &destination.token,
                &Method::POST,
                &uri,
                &headers,
                &body,
            )
            .map_err(|_| unavailable())?;
        let url = format!(
            "{}{}",
            destination.member.relay_address.trim_end_matches('/'),
            uri
        );
        let _reply_budget = relay
            .body_budget
            .clone()
            .try_acquire_many_owned(MAX_PEER_REPLY_BYTES as u32)
            .map_err(|_| unavailable())?;
        let response = tokio::time::timeout(Duration::from_secs(45), async {
            // Exactly one attempt. A lost reply may already have committed the
            // original key; never retry here or execute on this non-owner.
            let mut response = relay
                .client
                .post(url)
                .headers(headers)
                .header(PROOF, proof)
                .body(body)
                .send()
                .await
                .map_err(|_| unavailable())?;
            if !response.status().is_success() {
                return Err(unavailable());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
                if chunk.len() > MAX_PEER_REPLY_BYTES.saturating_sub(bytes.len()) {
                    return Err(unavailable());
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice::<ResponseEnvelope>(&bytes).map_err(|_| unavailable())
        })
        .await
        .map_err(|_| unavailable())
        .and_then(|value| value);
        let response = async {
            let response = response?;
            if response.owner.agent_id != id
                || response.owner.epoch == 0
                || response.owner.instance_id != destination.member.instance_id
                || (destination.token.epoch != 0 && response.owner != destination.token)
                || response.request_sha256 != request_sha256
            {
                return Err(unavailable());
            }
            bounded(relay.coordinator.store.check(&response.owner))
                .await
                .map_err(|_| unavailable())?;
            self.context(&call).await?;
            Ok::<_, CommandAdmissionError>(response)
        }
        .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                // The one mutation attempt may have committed. Recover only an
                // exact retained receipt; never repeat the mutation on either AM.
                if let OwnerCommandMutation::QueuedInput(submission) = mutation {
                    let fresh = self.context(&call).await?;
                    let retained = tokio::time::timeout(
                        Duration::from_secs(5),
                        Box::pin(
                            self.service()?
                                .lookup_queued_input(&fresh, credential, source, submission),
                        ),
                    )
                    .await
                    .map_err(|_| unavailable())??;
                    self.context(&call).await?;
                    if let Some(receipt) = retained {
                        return Ok(OwnerCommandReply::QueuedInput(Ok(receipt)));
                    }
                }
                // Missing receipt is not evidence of abort. The caller keeps
                // the original key/body; no new intent or local fallback is made.
                return Err(error);
            }
        };
        response.reply.map(Into::into).map_err(|error| error.0)
    }
}

use super::*;

fn positive(value: u64) -> bool {
    (1..=i64::MAX as u64).contains(&value)
}
impl ResponsesV2<'_> {
    /// Observe whether this exact source can be cancelled before turn binding.
    pub async fn observe_queued_cancellation(
        &self,
        agent_id: &str,
        response_id: &str,
    ) -> Result<QueuedCancellationObservation> {
        validation::input_id(agent_id)?;
        validation::input_id(response_id)?;
        let value: QueuedCancellationObservation = self
            .get(
                &[
                    "api",
                    "v2",
                    "agents",
                    agent_id,
                    "responses",
                    response_id,
                    "queued-cancellation",
                ],
                &[],
            )
            .await?;
        value.response.validate()?;
        if value.response.receipt.agent_id != agent_id
            || value.response.receipt.response_id.as_deref() != Some(response_id)
            || !positive(value.command_revision)
            || (value.cancellable
                && (!matches!(
                    value.response.status,
                    CommandState::Queued | CommandState::WaitingAction
                ) || value.response.turn_sequence.is_some()
                    || value.response.delivery.is_some()
                    || value.response.observed_cursor.is_some()
                    || value.response.failure.is_some()))
        {
            return Err(Error::protocol(
                "queued cancellation observation is inconsistent with its source",
            ));
        }
        Ok(value)
    }
    /// Cancel once with the required retained key. Recover uncertainty with the
    /// original route, body and key; never retarget a source after a lost reply.
    pub async fn cancel_queued_response(
        &self,
        agent_id: &str,
        response_id: &str,
        body: &QueuedCancellationRequest,
        idempotency_key: &str,
    ) -> Result<QueuedCancellation> {
        validation::input_id(agent_id)?;
        validation::input_id(response_id)?;
        validation::input_id(&body.source_command_id)?;
        if !positive(body.expected_command_revision) {
            return Err(Error::validation(
                "invalid queued cancellation source revision",
            ));
        }
        validation::input_id(idempotency_key)?;
        let header = HeaderValue::from_str(idempotency_key)
            .map_err(|_| Error::validation("idempotency key is not an HTTP header"))?;
        if idempotency_key.trim() != idempotency_key {
            return Err(Error::validation(
                "idempotency key must survive header normalization unchanged",
            ));
        }
        let segments = [
            "api",
            "v2",
            "agents",
            agent_id,
            "responses",
            response_id,
            "queued-cancellation",
        ];
        let mut request = Request::operation(Method::POST, &segments, Some(body), StatusCode::OK)?;
        request.root = true;
        request.headers.insert("idempotency-key", header);
        let bytes = http::send(&self.client.inner, request).await?;
        let value: QueuedCancellation = serde_json::from_slice(&bytes)
            .map_err(|_| Error::protocol("invalid queued cancellation receipt"))?;
        value.cancellation.validate()?;
        value.response.validate()?;
        let receipt = &value.cancellation;
        if receipt.receipt.response_id.is_some()
            || receipt.status != CommandState::Completed
            || receipt.turn_sequence.is_some()
            || receipt.delivery.is_some()
            || receipt.observed_cursor.is_some()
            || receipt.failure.is_some()
            || receipt.receipt.command_id == value.target.source_command_id
            || receipt.receipt.agent_id != agent_id
            || value.response.receipt.agent_id != agent_id
            || value.response.receipt.response_id.as_deref() != Some(response_id)
            || value.response.receipt.command_id != body.source_command_id
            || value.target.response_id != response_id
            || value.target.source_command_id != body.source_command_id
            || value.target.expected_command_revision != body.expected_command_revision
        {
            return Err(Error::protocol(
                "queued cancellation receipt did not match the original source intent",
            ));
        }
        Ok(value)
    }
}

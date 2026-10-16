use super::*;

impl ResponsesV2<'_> {
    /// Atomically request cancellation and admit a distinct replacement message.
    /// Persist the exact body/key before sending; retry uncertainty with the same
    /// target, observation and key. Other queued messages retain their order.
    pub async fn interrupt_with_input(
        &self,
        agent_id: &str,
        response_id: &str,
        body: &ResponseInterruptionRequest,
        idempotency_key: &str,
    ) -> Result<ResponseInterruption> {
        validation::input_id(agent_id)?;
        validation::input_id(response_id)?;
        ControlRequest {
            turn_sequence: body.turn_sequence,
            expected_agent_revision: body.expected_agent_revision,
            command: ControlCommand::Steer {
                text: body.text.clone(),
            },
        }
        .validate()?;
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
            "interrupt",
        ];
        let mut request =
            Request::operation(Method::POST, &segments, Some(body), StatusCode::ACCEPTED)?;
        request.root = true;
        request.headers.insert("idempotency-key", header);
        let bytes = http::send(&self.client.inner, request).await?;
        let value: ResponseInterruption = serde_json::from_slice(&bytes)
            .map_err(|_| Error::protocol("invalid response interruption receipt"))?;
        value.control.validate()?;
        value.response.validate()?;
        if value.control.receipt.agent_id != agent_id
            || value.response.receipt.agent_id != agent_id
            || value.target.response_id != response_id
            || value.target.turn_sequence != body.turn_sequence
            || value.expected_agent_revision != body.expected_agent_revision
            || value.control.receipt.response_id.is_some()
            || value.response.receipt.response_id.is_none()
            || value.response.receipt.response_id.as_deref() == Some(response_id)
            || value.control.receipt.command_id == value.response.receipt.command_id
            || value.control.receipt.agent_order.checked_add(1)
                != Some(value.response.receipt.agent_order)
            || value
                .control
                .turn_sequence
                .is_some_and(|turn| turn != body.turn_sequence)
        {
            return Err(Error::protocol(
                "interruption receipt did not match the original target and atomic admission",
            ));
        }
        Ok(value)
    }
}

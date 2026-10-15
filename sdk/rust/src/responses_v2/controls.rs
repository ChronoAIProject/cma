use super::*;
mod queued_cancellation;
mod queued_input;
mod validation;

impl ResponsesV2<'_> {
    /// Observe one exact response turn. Support is an observation, not caller permission.
    pub async fn observe_controls(
        &self,
        agent_id: &str,
        response_id: &str,
    ) -> Result<ResponseControls> {
        validation::input_id(agent_id)?;
        validation::input_id(response_id)?;
        let value: ResponseControls = self
            .get(
                &[
                    "api",
                    "v2",
                    "agents",
                    agent_id,
                    "responses",
                    response_id,
                    "controls",
                ],
                &[],
            )
            .await?;
        value.validate()?;
        if value.response.receipt.agent_id != agent_id
            || value.response.receipt.response_id.as_deref() != Some(response_id)
        {
            return Err(Error::protocol(
                "response control observation identity did not match the request",
            ));
        }
        Ok(value)
    }
    /// Submit once with a REQUIRED retained key. Recover an uncertain admission
    /// with this same body/key, including its original revision and turn.
    pub async fn submit_control(
        &self,
        agent_id: &str,
        response_id: &str,
        body: &ControlRequest,
        idempotency_key: &str,
    ) -> Result<ResponseControl> {
        validation::input_id(agent_id)?;
        validation::input_id(response_id)?;
        body.validate()?;
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
            "controls",
        ];
        let mut request =
            Request::operation(Method::POST, &segments, Some(body), StatusCode::ACCEPTED)?;
        request.root = true;
        request.headers.insert("idempotency-key", header);
        let bytes = http::send(&self.client.inner, request).await?;
        let value: ResponseControl = serde_json::from_slice(&bytes)
            .map_err(|_| Error::protocol("invalid response control receipt"))?;
        value.validate()?;
        if value.control.receipt.agent_id != agent_id
            || value.target.response_id != response_id
            || value.target.turn_sequence != body.turn_sequence
            || value.expected_agent_revision != body.expected_agent_revision
        {
            return Err(Error::protocol(
                "control receipt did not match the original target and observation",
            ));
        }
        Ok(value)
    }
    /// One bounded safe GET poll. Completed control means delivery; observe the
    /// target response separately to establish actual cancellation/completion.
    pub async fn read_command(&self, command_id: &str) -> Result<CommandStatus> {
        validation::input_id(command_id)?;
        let value: CommandStatus = self
            .get(&["api", "v2", "commands", command_id], &[])
            .await?;
        value.validate()?;
        identity(&value.receipt.command_id, command_id)?;
        Ok(value)
    }
}

use super::*;

fn positive(value: u64) -> bool {
    (1..=i64::MAX as u64).contains(&value)
}
impl ResponsesV2<'_> {
    /// Read all pending inputs in server admission order under current authority.
    pub async fn list_queued_inputs(&self, agent_id: &str) -> Result<QueuedInputPage> {
        validation::input_id(agent_id)?;
        let mut order = 0_u64;
        let mut ids = std::collections::HashSet::new();
        let mut data = Vec::new();
        for _ in 0..64 {
            let query = if order == 0 {
                vec![]
            } else {
                vec![("after", order.to_string())]
            };
            let value: QueuedInputPage = self
                .get(&["api", "v2", "agents", agent_id, "queued-inputs"], &query)
                .await?;
            if value.agent_id != agent_id
                || value.data.len() > 4
                || value.next_after.is_some_and(|next| {
                    value
                        .data
                        .last()
                        .is_none_or(|item| next != item.response.receipt.agent_order)
                })
            {
                return Err(Error::protocol(
                    "queued input page identity, size or cursor is invalid",
                ));
            }
            for item in &value.data {
                item.response.validate()?;
                if item.response.receipt.agent_id != agent_id
                    || item.response.receipt.response_id.is_none()
                    || item.response.receipt.agent_order <= order
                    || !ids.insert(item.response.receipt.command_id.clone())
                    || !positive(item.command_revision)
                    || !valid_text(&item.text)
                    || !matches!(
                        item.response.status,
                        CommandState::Queued | CommandState::WaitingAction
                    )
                    || item.response.turn_sequence.is_some()
                    || item.response.delivery.is_some()
                    || item.response.failure.is_some()
                {
                    return Err(Error::protocol(
                        "queued input item identity, order or status is invalid",
                    ));
                }
                order = item.response.receipt.agent_order;
            }
            data.extend(value.data);
            if data.len() > 64 {
                return Err(Error::protocol(
                    "queued input traversal exceeded admission bound",
                ));
            }
            if value.next_after.is_none() {
                return Ok(QueuedInputPage {
                    object: value.object,
                    agent_id: value.agent_id,
                    data,
                    next_after: None,
                });
            }
        }
        Err(Error::protocol("queued input traversal did not terminate"))
    }

    /// Replace pending input with the required retained key. Recover uncertainty with the
    /// original route, body and key; never retarget a source after a lost reply.
    pub async fn edit_queued_input(
        &self,
        agent_id: &str,
        response_id: &str,
        body: &QueuedInputRequest,
        idempotency_key: &str,
    ) -> Result<QueuedInput> {
        let header = validate_request(agent_id, response_id, body, idempotency_key)?;
        let segments = [
            "api",
            "v2",
            "agents",
            agent_id,
            "responses",
            response_id,
            "queued-input",
        ];
        let mut request = Request::operation(Method::POST, &segments, Some(body), StatusCode::OK)?;
        request.root = true;
        request.headers.insert("idempotency-key", header);
        let bytes = http::send(&self.client.inner, request).await?;
        let value: QueuedInput = serde_json::from_slice(&bytes)
            .map_err(|_| Error::protocol("invalid queued input edit receipt"))?;
        validate_receipt(&value, agent_id, response_id, body)?;
        Ok(value)
    }

    /// Read the original edit outcome once. Pending does not prove that it aborted.
    pub async fn read_queued_input_status(
        &self,
        agent_id: &str,
        response_id: &str,
        body: &QueuedInputRequest,
        idempotency_key: &str,
    ) -> Result<QueuedInputStatus> {
        let header = validate_request(agent_id, response_id, body, idempotency_key)?;
        let segments = [
            "api",
            "v2",
            "agents",
            agent_id,
            "responses",
            response_id,
            "queued-input",
            "status",
        ];
        let mut request = Request::operation(Method::POST, &segments, Some(body), StatusCode::OK)?;
        request
            .statuses
            .extend([StatusCode::ACCEPTED, StatusCode::SERVICE_UNAVAILABLE]);
        request.root = true;
        request.headers.insert("idempotency-key", header);
        let reply = http::receive(&self.client.inner, request).await?;
        if reply.status == StatusCode::OK {
            let value: QueuedInput = serde_json::from_slice(&reply.body)
                .map_err(|_| Error::protocol("invalid queued input recovery receipt"))?;
            validate_receipt(&value, agent_id, response_id, body)?;
            return Ok(QueuedInputStatus::Completed(Box::new(value)));
        }
        let request_id = reply
            .headers
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned);
        let retry_after = reply
            .headers
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(http::parse_retry_after);
        if reply.status == StatusCode::SERVICE_UNAVAILABLE {
            let raw: serde_json::Value = serde_json::from_slice(&reply.body).unwrap_or_default();
            if raw["error"]["code"] != "queued_input_outcome_pending" {
                return Err(http::problem_error(
                    reply.status,
                    request_id,
                    retry_after,
                    &reply.body,
                ));
            }
        }
        let pending: QueuedInputPending = serde_json::from_slice(&reply.body)
            .map_err(|_| Error::protocol("invalid queued input pending outcome"))?;
        if pending.error.message.trim().is_empty()
            || pending.error.code != "queued_input_outcome_pending"
            || pending.recovery.method != "POST"
            || pending.recovery.url
                != format!("/api/v2/agents/{agent_id}/responses/{response_id}/queued-input/status")
            || !pending.recovery.same_idempotency_key
            || !pending.recovery.same_body
        {
            return Err(Error::protocol(
                "queued input recovery changed the original route or intent",
            ));
        }
        if reply.status == StatusCode::SERVICE_UNAVAILABLE {
            return Err(Error::Api {
                status: 503,
                code: Some(pending.error.code),
                message: pending.error.message,
                request_id,
                retry_after,
                extensions: std::collections::BTreeMap::from([(
                    "recovery".into(),
                    serde_json::json!({
                        "method":pending.recovery.method,"url":pending.recovery.url,
                        "same_idempotency_key":true,"same_body":true
                    }),
                )]),
            });
        }
        Ok(QueuedInputStatus::Pending(Box::new(pending)))
    }
}

fn validate_request(
    agent_id: &str,
    response_id: &str,
    body: &QueuedInputRequest,
    idempotency_key: &str,
) -> Result<HeaderValue> {
    validation::input_id(agent_id)?;
    validation::input_id(response_id)?;
    validation::input_id(&body.source_command_id)?;
    if !positive(body.expected_command_revision) || !valid_text(&body.text) {
        return Err(Error::validation(
            "invalid queued input edit source revision",
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
    Ok(header)
}

fn validate_receipt(
    value: &QueuedInput,
    agent_id: &str,
    response_id: &str,
    body: &QueuedInputRequest,
) -> Result<()> {
    value.edit.validate()?;
    value.response.validate()?;
    let receipt = &value.edit;
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
        || value.command_revision
            != body
                .expected_command_revision
                .checked_add(1)
                .ok_or_else(|| Error::protocol("queued input revision overflow"))?
        || value.text != body.text
    {
        return Err(Error::protocol(
            "queued input edit receipt did not match the original source intent",
        ));
    }
    Ok(())
}

fn valid_text(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= 256 * 1024
}

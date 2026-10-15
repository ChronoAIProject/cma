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
        Ok(value)
    }
}

fn valid_text(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= 256 * 1024
}

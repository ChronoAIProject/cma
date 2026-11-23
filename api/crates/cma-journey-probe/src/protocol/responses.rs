use serde_json::Value;

use crate::model::{now, Intent, Reason};

use super::{
    bind, check_answer, http, set_header_identity, sse::Stream, submitted, Outcome, Protocol,
};

impl Protocol {
    pub(super) async fn responses_send(
        &self,
        intent: &Intent,
        outcome: &mut Outcome,
    ) -> Result<(), Reason> {
        let endpoint = format!("/api/v2/agents/{}/responses", intent.target);
        let response = self.post_cma(intent, &endpoint, true).await?;
        submitted(&response, outcome)?;
        set_header_identity(response.headers(), outcome);
        let id = outcome
            .evidence
            .response
            .as_deref()
            .ok_or(Reason::Contract)?;
        if outcome.evidence.command.is_none() {
            return Err(Reason::Contract);
        }
        let (complete, answer) = parse(Stream::open(response)?, id, &intent.target).await?;
        outcome.evidence.delivery_at = Some(now());
        if !complete {
            return Err(Reason::TerminalFailed);
        }
        outcome.evidence.result_digest = Some(check_answer(&answer, intent)?);
        Ok(())
    }

    pub(super) async fn responses_verify(
        &self,
        intent: &Intent,
        outcome: &mut Outcome,
    ) -> Result<(), Reason> {
        let path = format!("/api/v2/agents/{}/responses/receipt", intent.target);
        let admission = http::json(self.post_cma(intent, &path, false).await?).await?;
        if admission.is_null() {
            return Err(Reason::ReceiptMissing);
        }
        let (command, response) = receipt(&admission, &intent.target)?;
        bind(&mut outcome.evidence.command, command)?;
        bind(&mut outcome.evidence.response, response)?;
        outcome.evidence.receipt_at.get_or_insert_with(now);
        let command_record = self
            .response_json(&format!("/api/v2/commands/{command}"))
            .await?;
        if receipt(&command_record, &intent.target)? != (command, response)
            || command_record["receipt"] != admission["receipt"]
        {
            return Err(Reason::Contract);
        }
        let path = format!("/api/v2/responses/{response}");
        let result = self.response_json(&path).await?;
        identity(&result, response, &intent.target)?;
        match result["status"].as_str() {
            Some("completed") => {}
            Some("failed" | "incomplete" | "cancelled") => {
                outcome.settled = true;
                outcome.evidence.terminal_at.get_or_insert_with(now);
                return Err(Reason::TerminalFailed);
            }
            _ => return Err(Reason::TerminalMissing),
        }
        if result["completed_at"].as_i64().is_none() {
            return Err(Reason::Contract);
        }
        let mut output = String::new();
        let mut after: Option<String> = None;
        for _ in 0..8 {
            let mut endpoint = self.cma_url(&format!("{path}/output"));
            if let Some(after) = &after {
                endpoint.query_pairs_mut().append_pair("after", after);
            }
            let page =
                http::json(self.get_owner(endpoint, false, "application/json").await?).await?;
            http::same(&page["object"], "cma.response.output_page")?;
            http::same(&page["response_id"], response)?;
            append_output(&page["data"], &mut output)?;
            match page["has_more"].as_bool() {
                Some(false) if page["next_after"].is_null() => {
                    outcome.settled = true;
                    outcome.evidence.terminal_at.get_or_insert_with(now);
                    let digest = check_answer(&output, intent)?;
                    return bind(&mut outcome.evidence.result_digest, &digest);
                }
                Some(true) => {
                    let next = http::id(&page["next_after"])?;
                    if after.as_deref() == Some(next) {
                        return Err(Reason::Contract);
                    }
                    after = Some(next.to_owned());
                }
                _ => return Err(Reason::Contract),
            }
        }
        Err(Reason::Contract)
    }

    pub(super) async fn response_json(&self, path: &str) -> Result<Value, Reason> {
        http::json(
            self.get_owner(self.cma_url(path), false, "application/json")
                .await?,
        )
        .await
    }
}

fn receipt<'a>(record: &'a Value, agent: &str) -> Result<(&'a str, &'a str), Reason> {
    http::same(&record["object"], "cma.agent.command")?;
    let receipt = &record["receipt"];
    http::same(&receipt["agent_id"], agent)?;
    if receipt["agent_order"].as_u64().is_none()
        || receipt["event_cursor"].as_u64().is_none()
        || !super::timestamp(&receipt["admitted_at"])
    {
        return Err(Reason::Contract);
    }
    Ok((
        http::id(&receipt["command_id"])?,
        http::id(&receipt["response_id"])?,
    ))
}

fn identity(value: &Value, id: &str, agent: &str) -> Result<(), Reason> {
    http::same(&value["object"], "response")?;
    http::same(&value["id"], id)?;
    http::same(&value["conversation"]["id"], agent)
}

pub(super) fn append_output(items: &Value, output: &mut String) -> Result<(), Reason> {
    for item in items.as_array().ok_or(Reason::Contract)? {
        if item["type"] != "message" {
            continue;
        }
        http::same(&item["status"], "completed")?;
        http::same(&item["role"], "assistant")?;
        for part in item["content"].as_array().ok_or(Reason::Contract)? {
            http::same(&part["type"], "output_text")?;
            output.push_str(part["text"].as_str().ok_or(Reason::Contract)?);
            if output.len() > 32 * 1024 {
                return Err(Reason::Contract);
            }
        }
    }
    Ok(())
}

async fn parse(mut stream: Stream, id: &str, agent: &str) -> Result<(bool, String), Reason> {
    let mut sequence = 0;
    let mut active = false;
    let mut terminal = None;
    let mut items = Output::default();
    while let Some(frame) = stream.next().await? {
        let value = &frame.value;
        let kind = value["type"].as_str().ok_or(Reason::Contract)?;
        if frame.event.as_deref() != Some(kind) || terminal.is_some() {
            return Err(Reason::Contract);
        }
        if kind == "error" {
            return Err(Reason::DeliveryLost);
        }
        if value["sequence_number"].as_u64() != Some(sequence)
            || frame.id.as_deref() != Some(&sequence.to_string())
        {
            return Err(Reason::Contract);
        }
        if sequence == 0 && kind != "response.created" {
            return Err(Reason::Contract);
        }
        sequence += 1;
        match kind {
            "response.created" if sequence == 1 => {
                identity(&value["response"], id, agent)?;
                http::same(&value["response"]["status"], "queued")?;
            }
            "response.in_progress" if !active => {
                identity(&value["response"], id, agent)?;
                http::same(&value["response"]["status"], "in_progress")?;
                active = true;
            }
            "response.completed" | "response.failed" | "response.incomplete" => {
                identity(&value["response"], id, agent)?;
                http::same(
                    &value["response"]["status"],
                    kind.trim_start_matches("response."),
                )?;
                if items.current.is_some() {
                    return Err(Reason::Contract);
                }
                let complete = kind == "response.completed";
                if complete {
                    let mut terminal_output = String::new();
                    append_output(&value["response"]["output"], &mut terminal_output)?;
                    if terminal_output != items.answer {
                        return Err(Reason::Contract);
                    }
                }
                terminal = Some(complete);
            }
            _ if active => items.apply(kind, value)?,
            _ => return Err(Reason::Contract),
        }
    }
    Ok((terminal.ok_or(Reason::DeliveryLost)?, items.answer))
}

#[derive(Default)]
struct Output {
    next: u64,
    current: Option<Item>,
    answer: String,
}

struct Item {
    id: String,
    message: bool,
    phase: u8,
    text: String,
}

impl Output {
    fn apply(&mut self, kind: &str, value: &Value) -> Result<(), Reason> {
        if kind == "response.output_item.added"
            && self.current.is_none()
            && value["output_index"] == self.next
        {
            let item = &value["item"];
            self.current = Some(Item {
                id: http::id(&item["id"])?.to_owned(),
                message: item["type"] == "message",
                phase: 0,
                text: String::new(),
            });
            return Ok(());
        }
        let item = self.current.as_mut().ok_or(Reason::Contract)?;
        if value["output_index"] != self.next {
            return Err(Reason::Contract);
        }
        if kind == "response.output_item.done" {
            http::same(&value["item"]["id"], &item.id)?;
            if item.message {
                let mut text = String::new();
                append_output(&Value::Array(vec![value["item"].clone()]), &mut text)?;
                if item.phase != 3 || item.text != text {
                    return Err(Reason::Contract);
                }
                self.answer.push_str(&text);
            }
            self.current = None;
            self.next += 1;
            return Ok(());
        }
        http::same(&value["item_id"], &item.id)?;
        if !item.message && kind.starts_with("response.mcp_call.") {
            return Ok(());
        }
        if !item.message || value["content_index"] != 0 {
            return Err(Reason::Contract);
        }
        match (kind, item.phase) {
            ("response.content_part.added", 0) if value["part"]["type"] == "output_text" => {
                item.phase = 1
            }
            ("response.output_text.delta", 1) => item
                .text
                .push_str(value["delta"].as_str().ok_or(Reason::Contract)?),
            ("response.output_text.done", 1) if value["text"] == item.text => item.phase = 2,
            ("response.content_part.done", 2)
                if value["part"]["text"] == item.text && value["part"]["type"] == "output_text" =>
            {
                item.phase = 3
            }
            _ => return Err(Reason::Contract),
        }
        if item.text.len() + self.answer.len() > 32 * 1024 {
            return Err(Reason::Contract);
        }
        Ok(())
    }
}

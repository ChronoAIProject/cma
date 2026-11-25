use serde_json::Value;

use crate::model::{now, Intent, Reason};

use super::{
    bind, check_answer, digest, http, set_header_identity, sse::Stream, submitted, Outcome,
    Protocol,
};

struct Run {
    id: String,
    answer: String,
    complete: bool,
}

impl Protocol {
    pub(super) async fn agui_send(
        &self,
        intent: &Intent,
        outcome: &mut Outcome,
    ) -> Result<(), Reason> {
        let response = self.post_cma(intent, "/api/v1/agui/run", true).await?;
        submitted(&response, outcome)?;
        set_header_identity(response.headers(), outcome);
        if outcome.evidence.command.is_none() {
            return Err(Reason::Contract);
        }
        let run = parse(Stream::open(response)?, &intent.target, None).await?;
        bind(&mut outcome.evidence.conversation, &run.id)?;
        outcome.evidence.delivery_at = Some(now());
        if !run.complete {
            return Err(Reason::TerminalFailed);
        }
        outcome.evidence.result_digest = Some(check_answer(&run.answer, intent)?);
        Ok(())
    }

    pub(super) async fn agui_verify(
        &self,
        intent: &Intent,
        outcome: &mut Outcome,
    ) -> Result<(), Reason> {
        let response = self
            .post_cma(intent, "/api/v1/agui/run/receipt", false)
            .await?;
        let receipt = http::json(response).await?;
        if receipt.is_null() {
            return Err(Reason::ReceiptMissing);
        }
        http::same(&receipt["threadId"], &intent.target)?;
        outcome.evidence.receipt_at.get_or_insert_with(now);
        let run_id = http::id(&receipt["runId"]).map_err(|_| Reason::TerminalMissing)?;
        bind(&mut outcome.evidence.conversation, run_id)?;
        if let Some(command) = &outcome.evidence.command {
            let found = self
                .response_json(&format!("/api/v2/commands/{command}"))
                .await?;
            http::same(&found["object"], "cma.agent.command")?;
            http::same(&found["receipt"]["command_id"], command)?;
            http::same(&found["receipt"]["agent_id"], &intent.target)?;
            if found["turn_sequence"].as_u64() != run_id.parse::<u64>().ok() {
                return Err(Reason::Contract);
            }
            if let Some(response) = outcome.evidence.response.as_deref() {
                http::same(&found["receipt"]["response_id"], response)?;
            }
        }
        let complete = match receipt["status"].as_str() {
            Some("completed") => true,
            Some("failed" | "interrupted" | "cancelled") => false,
            _ => return Err(Reason::TerminalMissing),
        };
        let run = self.agui_retained(&intent.target, run_id, None).await?;
        if run.complete != complete {
            return Err(Reason::Contract);
        }
        outcome.settled = true;
        outcome.evidence.terminal_at.get_or_insert_with(now);
        if !complete {
            return Err(Reason::TerminalFailed);
        }
        let digest = check_answer(&run.answer, intent)?;
        bind(&mut outcome.evidence.result_digest, &digest)
    }

    pub(super) async fn agui_reference(&self) -> Result<(), Reason> {
        let r = &self.config.reference;
        let run = self.agui_retained(&r.agent, &r.run, Some(r.after)).await?;
        if !run.complete || digest(&run.answer) != digest(&r.answer) {
            return Err(Reason::Contract);
        }
        Ok(())
    }

    async fn agui_retained(
        &self,
        thread: &str,
        run: &str,
        after: Option<u64>,
    ) -> Result<Run, Reason> {
        let mut endpoint = self.cma_url("/api/v1/agui/events");
        endpoint
            .query_pairs_mut()
            .append_pair("thread_id", thread)
            .append_pair("follow", "false");
        if let Some(after) =
            after.or_else(|| run.parse::<u64>().ok().map(|turn| turn.saturating_sub(1)))
        {
            endpoint
                .query_pairs_mut()
                .append_pair("starting_after", &after.to_string());
        }
        let response = self.get_owner(endpoint, false, "text/event-stream").await?;
        parse(Stream::open(response)?, thread, Some(run)).await
    }
}

async fn parse(mut stream: Stream, thread: &str, selected: Option<&str>) -> Result<Run, Reason> {
    let mut run_id = selected.map(str::to_owned);
    let mut started = false;
    let mut terminal = None;
    let mut output = String::new();
    let mut message: Option<(String, bool)> = None;
    while let Some(frame) = stream.next().await? {
        let value = &frame.value;
        http::same(&value["threadId"], thread)?;
        let id = http::id(&value["runId"])?;
        if selected.is_some_and(|selected| selected != id) {
            continue;
        }
        if value["cma:live"] == Value::Bool(true) {
            continue;
        }
        bind(&mut run_id, id)?;
        match value["type"].as_str().ok_or(Reason::Contract)? {
            "RUN_STARTED" if !started && terminal.is_none() => started = true,
            "RUN_FINISHED" | "RUN_ERROR" if started && terminal.is_none() && message.is_none() => {
                terminal =
                    Some(value["type"] == "RUN_FINISHED" && value["outcome"]["type"] == "complete");
            }
            "TEXT_MESSAGE_START" if message.is_none() && terminal.is_none() => {
                let role = value["role"].as_str().ok_or(Reason::Contract)?;
                if !matches!(role, "user" | "assistant") {
                    return Err(Reason::Contract);
                }
                message = Some((
                    http::id(&value["messageId"])?.to_owned(),
                    role == "assistant",
                ));
            }
            "TEXT_MESSAGE_CONTENT" | "TEXT_MESSAGE_END" => {
                let (id, assistant) = message.as_ref().ok_or(Reason::Contract)?;
                http::same(&value["messageId"], id)?;
                if value["type"] == "TEXT_MESSAGE_END" {
                    message = None;
                } else {
                    let text = value["delta"].as_str().ok_or(Reason::Contract)?;
                    if *assistant {
                        output.push_str(text);
                    }
                    if output.len() > 32 * 1024 {
                        return Err(Reason::Contract);
                    }
                }
            }
            "RUN_STARTED" | "RUN_FINISHED" | "RUN_ERROR" | "TEXT_MESSAGE_START" => {
                return Err(Reason::Contract)
            }
            _ => {}
        }
    }
    let complete = terminal.ok_or(Reason::DeliveryLost)?;
    Ok(Run {
        id: run_id.ok_or(Reason::Contract)?,
        answer: output,
        complete,
    })
}

//! Bounded transcript continuation and compatibility hydration.
use super::{AgentTranscript, Agents, TranscriptRole, TranscriptTurn};
use crate::{Error, Result, resources::identity};
use serde::Deserialize;

/// A continuation bound to one captured source high-water mark.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TranscriptCursor {
    /// Numeric turn identity.
    pub turn: u64,
    /// Last consumed source event, including invisible events.
    pub after: u64,
    /// Captured source high-water mark.
    pub through: u64,
    /// Source events per page, from 1 through 32.
    pub limit: u16,
}

/// One page may contain only part of a long turn.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TranscriptPage {
    /// Page-local messages and repeated scalar turn metadata.
    #[serde(flatten)]
    pub transcript: AgentTranscript,
    /// Continue with all four fields, or stop when absent.
    pub next: Option<TranscriptCursor>,
}

impl Agents<'_> {
    /// Read a bounded page. A changed source or current authority refuses continuation.
    pub async fn transcript_page(
        &self,
        id: &str,
        limit: u16,
        cursor: Option<&TranscriptCursor>,
    ) -> Result<TranscriptPage> {
        if !(1..=32).contains(&limit) {
            return Err(Error::validation("transcript page limit must be 1..32"));
        }
        let mut query = vec![("limit", limit.to_string())];
        if let Some(cursor) = cursor {
            if !valid(cursor) || cursor.limit != limit {
                return Err(Error::validation("invalid transcript continuation"));
            }
            query.extend([
                ("turn", cursor.turn.to_string()),
                ("after", cursor.after.to_string()),
                ("through", cursor.through.to_string()),
            ]);
        }
        let page: TranscriptPage = self
            .client
            .get(&["agents", id, "transcript"], &query)
            .await?;
        identity(&page.transcript.agent_id, id)?;
        if cursor.is_some_and(|c| c.through != page.transcript.through_seq)
            || page.next.as_ref().is_some_and(|next| {
                !valid(next)
                    || next.limit != limit
                    || next.through != page.transcript.through_seq
                    || next.after <= cursor.map_or(0, |c| c.after)
            })
        {
            return Err(Error::protocol(
                "transcript continuation changed source or did not advance",
            ));
        }
        Ok(page)
    }

    pub(super) async fn complete_transcript(&self, id: &str) -> Result<AgentTranscript> {
        let result: Result<AgentTranscript> =
            self.client.get(&["agents", id, "transcript"], &[]).await;
        match result {
            Ok(value) => {
                identity(&value.agent_id, id)?;
                return Ok(value);
            }
            Err(Error::Api {
                code: Some(code), ..
            }) if code == "transcript_pagination_required" => {}
            Err(error) => return Err(error),
        }
        let mut next = None;
        let mut result: Option<AgentTranscript> = None;
        loop {
            let page = self.transcript_page(id, 32, next.as_ref()).await?;
            let target = result.get_or_insert_with(|| AgentTranscript {
                agent_id: id.into(),
                through_seq: page.transcript.through_seq,
                turns: vec![],
            });
            if target.through_seq != page.transcript.through_seq {
                return Err(Error::protocol("transcript snapshot changed"));
            }
            for mut turn in page.transcript.turns {
                match target.turns.last_mut() {
                    Some(previous) if previous.turn_sequence > turn.turn_sequence => {
                        return Err(Error::protocol("transcript turns moved backwards"));
                    }
                    Some(previous) if previous.turn_sequence == turn.turn_sequence => {
                        if previous.started_at != turn.started_at
                            || previous.owner_message != turn.owner_message
                            || previous.outcome != turn.outcome
                        {
                            return Err(Error::protocol("transcript turn metadata changed"));
                        }
                        validate_messages(
                            &turn,
                            previous.messages.last().map_or(0, |m| m.sequence),
                            target.through_seq,
                        )?;
                        previous.messages.append(&mut turn.messages);
                        previous.agent_messages.append(&mut turn.agent_messages);
                    }
                    _ => {
                        validate_messages(&turn, 0, target.through_seq)?;
                        target.turns.push(turn);
                    }
                }
            }
            next = page.next;
            if next.is_none() {
                return Ok(result.expect("the first page initializes the result"));
            }
        }
    }
}

fn valid(c: &TranscriptCursor) -> bool {
    (1..=32).contains(&c.limit)
        && c.turn > 0
        && c.through <= 9_007_199_254_740_991
        && c.after >= c.turn - 1
        && c.after < c.through
}
fn validate_messages(turn: &TranscriptTurn, mut after: u64, through: u64) -> Result<()> {
    for message in &turn.messages {
        if message.sequence <= after || message.sequence > through {
            return Err(Error::protocol("transcript message order is invalid"));
        }
        after = message.sequence;
    }
    if turn.agent_messages.iter().map(String::as_str).ne(turn
        .messages
        .iter()
        .filter(|m| m.role == TranscriptRole::Assistant)
        .map(|m| m.content.as_str()))
    {
        return Err(Error::protocol(
            "transcript assistant summaries disagree with source messages",
        ));
    }
    Ok(())
}

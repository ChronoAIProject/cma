//! The durable inbox receipt is visible before dispatch capacity or provider work.
use super::{
    inbox_reader::Body,
    journal::DurableJournal,
    journal_model::{Authority, Inbox},
    normalize::Normalized,
    GatewayError,
};

impl DurableJournal {
    pub(super) async fn observe_inbox(
        &self,
        inbox: &Inbox,
        authority: &Authority,
        bytes: &[u8],
    ) -> Result<(), GatewayError> {
        let body: Body = serde_json::from_slice(bytes).map_err(|_| GatewayError::Protocol)?;
        if body.channel.record_id.as_str() != authority.record
            || body.channel.owner.subject != authority.owner
            || body.dispatch["id"].as_str() != Some(&inbox.message_id)
        {
            return Err(GatewayError::Authority);
        }
        // Refused or malformed intake remains the dispatcher's durable refusal;
        // it does not acquire an ordinary Activity or block the Gateway sequence.
        let Ok(Normalized::Activity(activity)) = body.activity(&inbox.bot_id) else {
            return Ok(());
        };
        self.activity
            .receive_at(
                &authority.owner,
                &body.channel.record_id,
                &activity,
                inbox.received_at,
            )
            .await
            .map_err(|_| GatewayError::Unavailable)
    }
}

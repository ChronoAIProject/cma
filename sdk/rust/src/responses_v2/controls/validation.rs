use super::*;

fn bounded(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
fn positive(value: u64) -> bool {
    (1..=i64::MAX as u64).contains(&value)
}
pub(super) fn input_id(value: &str) -> Result<()> {
    if !bounded(value, 256) {
        return Err(Error::validation(
            "invalid response control identifier or key",
        ));
    }
    Ok(())
}
impl ControlRequest {
    pub(super) fn validate(&self) -> Result<()> {
        let command = match &self.command {
            ControlCommand::Cancel {} => true,
            ControlCommand::Steer { text } => !text.trim().is_empty() && text.len() <= 256 * 1024,
            ControlCommand::ResolveApproval { approval_id, .. } => bounded(approval_id, 512),
        };
        if !positive(self.turn_sequence) || !positive(self.expected_agent_revision) || !command {
            return Err(Error::validation("invalid control intent or observation"));
        }
        Ok(())
    }
}
impl CommandStatus {
    pub(super) fn validate(&self) -> Result<()> {
        if !bounded(&self.receipt.command_id, 256)
            || !bounded(&self.receipt.agent_id, 256)
            || self
                .receipt
                .response_id
                .as_ref()
                .is_some_and(|value| !bounded(value, 256))
            || !bounded(&self.receipt.admitted_at, 256)
            || !bounded(&self.queue_deadline, 256)
            || !positive(self.receipt.agent_order)
            || self.receipt.event_cursor > i64::MAX as u64
            || self.turn_sequence.is_some_and(|value| !positive(value))
            || self
                .observed_cursor
                .is_some_and(|value| value > i64::MAX as u64)
            || (self.delivery.is_some() && self.turn_sequence.is_none())
            || (self.observed_cursor.is_some() && self.delivery.is_none())
            || (self.status == CommandState::Queued
                && (self.turn_sequence.is_some() || self.failure.is_some()))
        {
            return Err(Error::protocol(
                "command identity, numeric range or delivery is inconsistent",
            ));
        }
        Ok(())
    }
}
impl ResponseControlTarget {
    fn valid(&self) -> bool {
        bounded(&self.response_id, 256) && positive(self.turn_sequence)
    }
}
impl ResponseControls {
    pub(super) fn validate(&self) -> Result<()> {
        self.response.validate()?;
        if !positive(self.agent_revision)
            || self.response.receipt.response_id.is_none()
            || self.target.is_none() != self.response.turn_sequence.is_none()
            || self.target.as_ref().is_some_and(|target| {
                !target.valid()
                    || Some(target.response_id.as_str())
                        != self.response.receipt.response_id.as_deref()
                    || Some(target.turn_sequence) != self.response.turn_sequence
            })
            || (self.target_active && (self.target.is_none() || self.response.status.is_terminal()))
        {
            return Err(Error::protocol(
                "response control observation does not match its source command",
            ));
        }
        Ok(())
    }
}
impl ResponseControl {
    pub(super) fn validate(&self) -> Result<()> {
        self.control.validate()?;
        if !positive(self.expected_agent_revision)
            || !self.target.valid()
            || self.control.receipt.response_id.is_some()
            || self
                .control
                .turn_sequence
                .is_some_and(|turn| turn != self.target.turn_sequence)
        {
            return Err(Error::protocol("control receipt target is inconsistent"));
        }
        Ok(())
    }
}

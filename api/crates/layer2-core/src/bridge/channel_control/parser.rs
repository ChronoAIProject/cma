use serde::{Deserialize, Serialize};

use crate::sandbox::agent_io::{AgentCommand, AgentContent, AgentInput};

use super::ChannelControlError;

/// Data only. There is deliberately no approval operation in this protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChannelControlCommand {
    Cancel,
    Steer { input: AgentInput },
}

impl ChannelControlCommand {
    pub fn command(&self) -> AgentCommand {
        match self {
            Self::Cancel => AgentCommand::Interrupt,
            Self::Steer { input } => AgentCommand::Steer {
                input: input.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelControlRequest {
    pub reference: String,
    pub command: ChannelControlCommand,
}

impl ChannelControlRequest {
    pub fn validate(&self) -> Result<(), ChannelControlError> {
        if !self.reference.strip_prefix("ccr_").is_some_and(|suffix| {
            suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) {
            return Err(ChannelControlError::Invalid);
        }
        if let ChannelControlCommand::Steer { input } = &self.command {
            if input
                .canonical()
                .map_err(|_| ChannelControlError::Invalid)?
                != *input
                || !matches!(input.content.as_slice(), [AgentContent::Text { .. }])
            {
                return Err(ChannelControlError::Invalid);
            }
        }
        Ok(())
    }

    /// The executable-intent grammar requires an actual issued reference shape.
    /// The older disabled-prefix guard intentionally keeps its looser grammar.
    pub fn parse(text: &str) -> Result<Self, ChannelControlError> {
        if text.len() > 65_536 {
            return Err(ChannelControlError::Invalid);
        }
        let (header, payload) = text
            .split_once('\n')
            .map_or((text, None), |(header, payload)| (header, Some(payload)));
        let mut words = header.split(' ');
        if words.next() != Some("CMA-CONTROL/1") {
            return Err(ChannelControlError::Invalid);
        }
        let operation = words.next().ok_or(ChannelControlError::Invalid)?;
        let reference = words.next().ok_or(ChannelControlError::Invalid)?;
        if words.next().is_some()
            || !reference.strip_prefix("ccr_").is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err(ChannelControlError::Invalid);
        }
        let command = match (operation, payload) {
            ("cancel", None) => ChannelControlCommand::Cancel,
            ("steer", Some(text)) => ChannelControlCommand::Steer {
                input: AgentInput::text(text)
                    .canonical()
                    .map_err(|_| ChannelControlError::Invalid)?,
            },
            _ => return Err(ChannelControlError::Invalid),
        };
        let result = Self {
            reference: reference.into(),
            command,
        };
        result.validate()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE: &str = "ccr_0123456789abcdef0123456789abcdef";

    #[test]
    fn exact_protocol_preserves_multiline_input_and_only_two_operations() {
        let cancel =
            ChannelControlRequest::parse(&format!("CMA-CONTROL/1 cancel {REFERENCE}")).unwrap();
        assert_eq!(cancel.command.command(), AgentCommand::Interrupt);
        let steer = ChannelControlRequest::parse(&format!(
            "CMA-CONTROL/1 steer {REFERENCE}\nUse the smaller scope.\nKeep the original output."
        ))
        .unwrap();
        assert_eq!(
            steer.command.command(),
            AgentCommand::Steer {
                input: AgentInput::text("Use the smaller scope.\nKeep the original output.")
            }
        );
        for text in [
            format!(" CMA-CONTROL/1 cancel {REFERENCE}"),
            format!("CMA-CONTROL/1  cancel {REFERENCE}"),
            format!("CMA-CONTROL/1 cancel {REFERENCE}\n"),
            format!("CMA-CONTROL/1 steer {REFERENCE}\n \n"),
            format!("CMA-CONTROL/2 cancel {REFERENCE}"),
            format!("CMA-CONTROL/1 approve {REFERENCE}"),
            format!("CMA-CONTROL/1 cancel {REFERENCE} extra"),
            "CMA-CONTROL/1 cancel ccr_source".into(),
        ] {
            assert_eq!(
                ChannelControlRequest::parse(&text),
                Err(ChannelControlError::Invalid)
            );
        }
    }
}

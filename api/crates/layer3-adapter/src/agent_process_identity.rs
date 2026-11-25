/// The first process uses the original Agent-lifetime operation IDs. Each
/// subsequent process pairs its Open and Close under one incarnation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentProcessAction {
    Open,
    Close,
}

impl AgentProcessAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Close => "close",
        }
    }

    pub fn prefix(self, agent: &str) -> String {
        format!("agent:{agent}:{}:", self.as_str())
    }

    pub fn operation_id(self, agent: &str, incarnation: u64) -> String {
        let prefix = self.prefix(agent);
        if incarnation == 1 {
            format!("{prefix}v1")
        } else {
            format!("{prefix}{incarnation}:v1")
        }
    }

    pub fn incarnation(self, agent: &str, operation_id: &str) -> Option<u64> {
        if operation_id.len() > 256 {
            return None;
        }
        let suffix = operation_id.strip_prefix(&self.prefix(agent))?;
        if suffix == "v1" {
            return Some(1);
        }
        let number = suffix.strip_suffix(":v1")?;
        if number.starts_with('0') || !number.bytes().all(|digit| digit.is_ascii_digit()) {
            return None;
        }
        number
            .parse::<u64>()
            .ok()
            .filter(|incarnation| *incarnation > 1)
    }
}

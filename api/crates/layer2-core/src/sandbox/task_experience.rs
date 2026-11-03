use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskExperienceCapability {
    TaskHomeAdmission,
    SuggestedFollowupAdmission,
    DesktopEnable,
    BrowserTakeover,
    ReplayPresentation,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Mode {
    #[default]
    Disabled,
    Accounts,
    All,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(try_from = "Configuration")]
pub struct TaskExperiencePolicy {
    mode: Mode,
    accounts: BTreeSet<String>,
    capabilities: BTreeSet<TaskExperienceCapability>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    mode: Mode,
    accounts: Vec<String>,
    capabilities: Vec<TaskExperienceCapability>,
}

impl TryFrom<Configuration> for TaskExperiencePolicy {
    type Error = &'static str;

    fn try_from(value: Configuration) -> Result<Self, Self::Error> {
        if value.accounts.len() > 1000
            || value.accounts.iter().any(|account| {
                account.is_empty()
                    || account.len() > 256
                    || account
                        .chars()
                        .any(|character| character.is_control() || character.is_whitespace())
            })
            || (value.mode != Mode::Accounts && !value.accounts.is_empty())
            || (value.mode == Mode::Disabled && !value.capabilities.is_empty())
        {
            return Err("invalid task experience policy selection");
        }
        let accounts: BTreeSet<_> = value.accounts.iter().cloned().collect();
        let capabilities: BTreeSet<_> = value.capabilities.iter().copied().collect();
        if accounts.len() != value.accounts.len() || capabilities.len() != value.capabilities.len()
        {
            return Err("duplicate task experience policy selection");
        }
        Ok(Self {
            mode: value.mode,
            accounts,
            capabilities,
        })
    }
}

impl TaskExperiencePolicy {
    pub fn allows(&self, account: &crate::SubjectId, capability: TaskExperienceCapability) -> bool {
        self.capabilities.contains(&capability)
            && match self.mode {
                Mode::Disabled => false,
                Mode::Accounts => self.accounts.contains(account.as_str()),
                Mode::All => true,
            }
    }
}

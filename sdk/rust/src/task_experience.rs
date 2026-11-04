#![doc = "Authenticated task experience policy, independent of environment capability."]
use crate::{
    Client, Error, Result,
    http::{self, Request},
};
use serde::Deserialize;

#[doc = "Current caller's independently selected capabilities; never a retained grant."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TaskExperienceCapabilities {
    #[doc = "Admit a new initial task during Agent creation."]
    pub task_home_admission: bool,
    #[doc = "Admit a new answer-scoped follow-up."]
    pub suggested_followup_admission: bool,
    #[doc = "Enable a browser, subject to independently verified environment support."]
    pub desktop_enable: bool,
    #[doc = "Acquire browser control, input or renew; recovery and return remain authorized separately."]
    pub browser_takeover: bool,
    #[doc = "Present operation replay; retained history access is unchanged."]
    pub replay_presentation: bool,
}

#[doc = "Bounded public task experience policy response."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TaskExperience {
    #[doc = "Exactly cma.ui.task_experience."]
    pub object: String,
    #[doc = "Supported schema version, currently1."]
    pub schema_version: u16,
    #[doc = "Authenticated caller identity for fencing delayed reads and account changes."]
    pub account_id: String,
    #[doc = "Only this authenticated caller's eligibility; no account cohort identifiers."]
    pub capabilities: TaskExperienceCapabilities,
}

impl Client {
    #[doc = "Read fresh caller policy without caching; errors never authorize new admission."]
    pub async fn read_task_experience(&self) -> Result<TaskExperience> {
        let mut request = Request::get(&["api", "v2", "ui", "task-experience"]);
        request.root = true;
        let received = http::receive_bounded(&self.inner, request, 4096).await?;
        let value: TaskExperience = serde_json::from_slice(&received.body)
            .map_err(|_| Error::protocol("invalid task experience policy"))?;
        if value.object != "cma.ui.task_experience"
            || value.schema_version != 1
            || value.account_id.is_empty()
        {
            return Err(Error::protocol("unsupported task experience policy"));
        }
        Ok(value)
    }
}

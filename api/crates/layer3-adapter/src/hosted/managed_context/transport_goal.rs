use super::*;

impl HostedContextRefresh {
    /// Reattachment need not replay an already acknowledged native goal snapshot.
    /// Read only the exact retained home, through the existing Pod/boot-fenced
    /// maintenance transport. Unknown schemas and any retained goal defer work.
    pub(in crate::hosted) async fn transport_goal_absent(
        &self,
        session: &str,
    ) -> crate::Result<()> {
        let before = self.status().await?;
        let home = self.0.path.strip_suffix("/AGENTS.md").ok_or_else(refused)?;
        let command = super::super::RuntimeRecoveryCommand {
            program: "/usr/bin/python3".into(),
            args: vec![
                "-I".into(),
                "-c".into(),
                include_str!("transport_goal.py").into(),
            ],
            input: serde_json::to_vec(&serde_json::json!({
                "home":format!("/home/{home}"), "session":session,
            }))
            .map_err(|_| refused())?,
        };
        let output = self
            .0
            .backend
            .run_recovery_command(&self.0.handle, &command)
            .await?;
        let result: serde_json::Value = serde_json::from_slice(&output).map_err(|_| refused())?;
        if result != serde_json::json!({"version":1,"session":session,"goal_absent":true})
            || self.status().await? != before
        {
            return Err(refused());
        }
        Ok(())
    }
}

fn refused() -> AdapterError {
    AdapterError::Unsupported("retained native goal state does not permit transport refresh".into())
}

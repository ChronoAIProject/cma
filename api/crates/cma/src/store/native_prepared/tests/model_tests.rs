use super::*;

#[test]
fn native_prepared_normalizes_only_private_references() {
    let plan = plan();
    let (sandbox, workspace) = actual_profiles(&plan);
    let agent = plan.warm.workspaces[0].agents[0].content.clone();
    assert!(plan.matches(&sandbox, &workspace, &agent, &[]));
    for case in 0..9 {
        let (mut s, mut w, mut a) = (sandbox.clone(), workspace.clone(), agent.clone());
        let mut reaches = vec![];
        match case {
            0 => s.content.cpu_millis += 1,
            1 => s.content.runtime_image = Some(plan.warm.runtime_version.clone()),
            2 => w.content.setup_commands.push("echo surprise".into()),
            3 => {
                w.content.agent_profiles = plan.warm.workspaces[0]
                    .profile
                    .content
                    .agent_profiles
                    .clone()
            }
            4 => a.model = Some("different".into()),
            5 => a.permission_level = PermissionLevel::Guarded,
            6 => s.content.workspace_profiles[0].revision += 1,
            7 => reaches.push(ServiceId::new("extra")),
            _ => a.system_prompt.clear(),
        }
        s.digest = content_digest(&s.content).unwrap();
        w.digest = content_digest(&w.content).unwrap();
        assert!(!plan.matches(&s, &w, &a, &reaches), "case {case}");
    }
}

#[test]
fn native_prepared_accepts_exact_managed_instructions_and_retains_legacy_profiles() {
    let mut next = plan();
    let mut legacy = next.clone();
    let agent = &mut legacy.warm.workspaces[0].agents[0];
    agent.content.system_prompt.clear();
    agent.digest = content_digest(&agent.content).unwrap();
    assert!(next.validate().is_ok());
    assert!(legacy.validate().is_ok());
    let (sandbox, workspace) = actual_profiles(&next);
    assert!(next.matches(&sandbox, &workspace, &agent_content(&next), &[]));
    assert!(!legacy.matches(&sandbox, &workspace, &agent_content(&next), &[]));
    let agent = &mut next.warm.workspaces[0].agents[0];
    agent
        .content
        .system_prompt
        .push_str("\nUnreviewed instruction");
    agent.digest = content_digest(&agent.content).unwrap();
    assert!(next.validate().is_err());

    fn agent_content(plan: &NativePreparedPlan) -> AgentProfileContent {
        plan.warm.workspaces[0].agents[0].content.clone()
    }
}

#[test]
fn native_prepared_record_rejects_unpinned_runtime_home_and_query_fields() {
    let p = plan();
    let record = ready(Record {
        payload: WarmSandbox::new(p.warm.clone(), NOW.into()).unwrap(),
        plan: p,
        ownership: None,
    });
    for case in 0..5 {
        let mut changed = record.clone();
        match case {
            0 => changed.payload.home = None,
            1 => changed.payload.runtime.as_mut().unwrap().boot_id = None,
            2 => {
                changed
                    .payload
                    .home
                    .as_mut()
                    .unwrap()
                    .native_pool
                    .as_mut()
                    .unwrap()
                    .request
                    .pool
                    .spec
                    .cpu_millis += 1
            }
            3 => changed.payload.state = WarmSandboxState::Claimed,
            _ => changed.payload.plan.execution.clone_depth += 1,
        }
        assert!(changed.validate().is_err(), "case {case}");
    }
    let mut value = document(&record).unwrap();
    value.insert("state", "claimed");
    assert!(read(&value).is_err());
}

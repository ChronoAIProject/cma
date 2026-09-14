use cma_sdk::profiles::ProfileKind;
use serde_json::{Value, json};

pub const NOW: &str = "2026-09-06T01:00:00Z";

pub fn reference() -> Value {
    json!({"id":"profile-1", "revision":1})
}

pub fn content(kind: ProfileKind) -> Value {
    match kind {
        ProfileKind::Agent => {
            json!({"thinking_effort":"high", "permission_level":"guarded", "skills":[], "system_prompt":"Review changes."})
        }
        ProfileKind::Workspace => {
            json!({"location":{"type":"directory","name":"project"}, "name":"project", "setup_commands":[], "agent_profiles":[]})
        }
        ProfileKind::Sandbox => {
            json!({"cpu_millis":1000, "memory_mib":1024, "nyxid_services":[], "agent_provider":"codex", "model_access":"chronoai_managed", "max_workspaces":2, "max_agents_per_workspace":2, "workspace_profiles":[], "software_install_commands":[], "environment_variables":[]})
        }
    }
}

pub fn profile(kind: ProfileKind) -> Value {
    json!({"id":format!("{}-1",kind.as_str()), "owner":"owner-1", "name":"Project", "description":"A test profile", "visibility":"private", "uses_count":0, "draft":content(kind), "draft_version":1, "created_at":NOW, "updated_at":NOW})
}

pub fn revision(kind: ProfileKind) -> Value {
    json!({"profile_id":format!("{}-1",kind.as_str()), "revision":1, "content":content(kind), "digest":"digest-1", "published_at":NOW})
}

pub fn marketplace(kind: ProfileKind) -> Value {
    let mut value = json!({"summary":{"kind":kind, "id":format!("{}-1",kind.as_str()), "name":"Project", "description":"A test profile", "owner_display":"Owner", "latest_revision":1, "published_at":NOW, "uses_count":0}, "revision":revision(kind), "resolved_profiles":[], "resolved_revisions":[]});
    if kind == ProfileKind::Sandbox {
        value["trust"] = json!({"requires_confirmation":true,"commands_digest":"digest-1"});
    }
    value
}

pub fn sandbox() -> Value {
    json!({"id":"sandbox-1", "name":"Project", "status":"active", "profile":reference(), "limits":{"max_workspaces":2,"max_agents_per_workspace":2}, "last_activity_at":NOW, "created_at":NOW, "updated_at":NOW, "revision":0})
}

pub fn sandbox_summary() -> Value {
    let mut value = sandbox();
    value["workspace_count"] = json!(1);
    value["worktree_count"] = json!(1);
    value["agent_count"] = json!(1);
    value
}

pub fn sandbox_detail() -> Value {
    let mut value = sandbox_summary();
    value["workspaces"] = json!([workspace_summary()]);
    value
}

pub fn workspace() -> Value {
    json!({"id":"workspace-1", "sandbox_id":"sandbox-1", "name":"Project", "path":"/workspace/project", "source":{"type":"empty"}, "status":"ready", "profile":reference(), "created_at":NOW, "revision":0})
}

pub fn workspace_summary() -> Value {
    json!({"id":"workspace-1","sandbox_id":"sandbox-1","name":"Project","status":"ready","profile":reference(),"worktree_count":1,"agent_count":1})
}

pub fn worktree() -> Value {
    json!({"id":"worktree-1","workspace_id":"workspace-1","sandbox_id":"sandbox-1","name":"Agent tree","path":"/workspace/trees/agent-1","branch":"agent-1","base_commit":"commit-1","status":"ready","agent_id":"agent-1"})
}

pub fn workspace_detail() -> Value {
    let mut value = workspace();
    value["worktrees"] = json!([worktree()]);
    value["agents"] = json!([{"id":"agent-1","worktree_id":"worktree-1","workspace_id":"workspace-1","sandbox_id":"sandbox-1","status":"ready","profile":reference(),"updated_at":NOW}]);
    value["worktree_count"] = json!(1);
    value["agent_count"] = json!(1);
    value
}

pub fn agent_profile() -> Value {
    json!({"id":"profile-1","revision":1,"digest":"digest-1","thinking_effort":"high","permission_level":"guarded","skills":[]})
}

pub fn agent_summary() -> Value {
    json!({"id":"agent-1","sandbox_id":"sandbox-1","workspace_id":"workspace-1","worktree_id":"worktree-1","status":"ready","profile":agent_profile(),"created_at":NOW,"updated_at":NOW})
}

pub fn agent() -> Value {
    json!({"id":"agent-1","status":"ready","profile":agent_profile(),
        "worktree":{"id":"worktree-1","name":"Agent tree","path":"/workspace/trees/agent-1","branch":"agent-1","base_commit":"commit-1","status":"ready"},
        "workspace":{"id":"workspace-1","name":"Project","path":"/workspace/project","status":"ready","profile":{"id":"profile-1","revision":1,"digest":"digest-1"}},
        "sandbox":{"id":"sandbox-1","name":"Project","status":"active","profile":{"id":"profile-1","revision":1,"digest":"digest-1"}},
        "conforms_to":["https://spec.chrono-ai.fun/cma/1.0/conf/conversation"],"reach":[],"created_at":NOW,"updated_at":NOW})
}

pub fn resources() -> Value {
    json!({"agent_id":"agent-1","sandbox_id":"sandbox-1","uptime_ms":0,"cpu_usage_usec":null,"cpu_millis_limit":null,"memory_bytes_used":null,"memory_bytes_limit":null,"memory_bytes_peak":null,"stall_permille":null,"ceiling_hits":null,"allocation_failures":null,"kills":null,"received_bytes":null,"sent_bytes":null})
}

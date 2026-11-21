//! Project only reference pins and the authority needed to classify a survivor.
pub(super) fn sources() -> Vec<(&'static str, Option<&'static str>, Vec<&'static str>)> {
    let worker = vec![
        "record.source.sandbox_profile",
        "record.source.workspace_profile",
        "record.source.agent_profile",
        "record.profiles.sandbox",
        "record.profiles.workspace",
        "record.profiles.agent",
    ];
    vec![
        ("sandboxes", Some("owner"), vec!["record.profile"]),
        ("workspaces", Some("owner"), vec!["record.profile"]),
        ("agents", Some("owner"), vec!["record.profile"]),
        (
            "bots",
            Some("owner"),
            vec![
                "bot.profiles.sandbox",
                "bot.profiles.workspace",
                "bot.profiles.agent",
            ],
        ),
        (
            "trigger_definitions",
            Some("owner"),
            vec!["record.draft.target", "record.revisions.content.target"],
        ),
        (
            "trigger_definition_revisions",
            Some("owner"),
            vec!["record.content.target"],
        ),
        ("event_worker_profiles", Some("owner"), worker.clone()),
        ("event_worker_profile_revisions", Some("owner"), worker),
        ("platform_settings", None, vec!["settings.defaults"]),
    ]
}

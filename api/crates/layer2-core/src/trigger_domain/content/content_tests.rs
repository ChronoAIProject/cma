use super::*;

mod quick_message_tests {
    use super::*;
    use crate::trigger_domain::{trigger_content_digest, TriggerRevision};

    fn legacy() -> serde_json::Value {
        serde_json::json!({
            "presentation": {"title":"Example", "description":"", "button_label":"run_with_cma", "theme":"auto"},
            "target": {"sandbox_profile":{"id":"sbp_example","revision":1}, "workspace_profile":{"id":"wkp_example","revision":1}, "agent_profile":{"id":"agp_example","revision":1}},
            "reuse":{"sandbox":"reuse_existing","workspace":"reuse_existing","agent":"resume_existing"},
            "first_message":"Hello", "consent":{"always_review":false}
        })
    }

    fn message() -> TriggerQuickMessage {
        TriggerQuickMessage {
            name: "Introduce the project".into(),
            description: "Describe the project and its uses.".into(),
            text: "Read the repository introduction and describe it.".into(),
        }
    }

    #[test]
    fn empty_shortcuts_preserve_legacy_serialization_and_revision_digests() {
        let original = legacy();
        let content: TriggerContent = serde_json::from_value(original.clone()).unwrap();
        assert!(content.quick_messages.is_empty());
        assert_eq!(serde_json::to_value(&content).unwrap(), original);
        let mut explicit = original.clone();
        explicit["quick_messages"] = serde_json::json!([]);
        let explicit: TriggerContent = serde_json::from_value(explicit).unwrap();
        assert_eq!(
            trigger_content_digest(&content).unwrap(),
            trigger_content_digest(&explicit).unwrap()
        );
        let revision = TriggerRevision::new(1, content, "2026-09-09T00:00:00Z".into()).unwrap();
        revision.validate().unwrap();
        let mut changed = revision.clone();
        changed.content.quick_messages.push(message());
        assert_eq!(
            changed.validate().unwrap_err().code,
            "trigger_digest_mismatch"
        );
    }

    #[test]
    fn shortcuts_require_bounded_unique_names_descriptions_and_text() {
        let mut content: TriggerContent = serde_json::from_value(legacy()).unwrap();
        content.quick_messages.push(message());
        content.validate().unwrap();
        for (field, invalid) in [
            ("name", " ".into()),
            ("name", "x".repeat(121)),
            ("name", "line\nbreak".into()),
            ("description", " ".into()),
            ("description", "x".repeat(2001)),
            ("text", " ".into()),
            ("text", "x".repeat(32769)),
        ] {
            let mut value = serde_json::to_value(&content).unwrap();
            value["quick_messages"][0][field] = serde_json::Value::String(invalid);
            assert!(
                serde_json::from_value::<TriggerContent>(value)
                    .unwrap()
                    .validate()
                    .is_err(),
                "{field}"
            );
        }
        let mut duplicate = message();
        duplicate.name = " INTRODUCE THE PROJECT ".into();
        content.quick_messages.push(duplicate);
        assert_eq!(
            content.validate().unwrap_err().code,
            "duplicate_quick_message_name"
        );
        content.quick_messages = (0..9)
            .map(|index| TriggerQuickMessage {
                name: index.to_string(),
                ..message()
            })
            .collect();
        assert_eq!(
            content.validate().unwrap_err().code,
            "too_many_quick_messages"
        );
    }
}

mod inline_target_tests {
    use super::*;

    fn inline_content() -> TriggerContent {
        serde_json::from_value(serde_json::json!({
            "presentation": {
                "title": "Inline Trigger",
                "description": "",
                "button_label": "try_it",
                "theme": "dark"
            },
            "target": {
                "inline": {
                    "image": {
                        "id": "private-agent",
                        "version": "v3",
                        "reference": "registry.example/agent@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    },
                    "agent": {
                        "model": "gpt-5-agent",
                        "reasoning_effort": "high",
                        "permission_mode": "guarded",
                        "skills": [{"source": "ornn", "id": "repo-search"}],
                        "system_prompt": "Keep changes small.",
                        "services": ["api-github"]
                    },
                    "workspace": {
                        "repository": {
                            "url": "https://github.com/chronoai/project",
                            "reference": "main"
                        }
                    }
                }
            },
            "reuse": {"sandbox": "always_new", "workspace": "always_new", "agent": "always_new"},
            "first_message": "Inspect the repository",
            "consent": {"always_review": true}
        }))
        .expect("inline trigger JSON should decode")
    }

    #[test]
    fn inline_target_validates_without_profile_references() {
        inline_content()
            .validate()
            .expect("inline target should validate without legacy profiles");
    }

    #[test]
    fn custom_image_selection_is_refused_without_invalidating_retained_content() {
        let content = inline_content();
        content.validate().expect("retained inline image is valid");
        let error = content
            .reject_custom_image_selection()
            .expect_err("new custom image selection must be refused");
        assert_eq!(error.code(), "trigger_image_selection_unsupported");
        assert_eq!(error.field(), "target.inline.image");
    }

    #[test]
    fn system_default_and_profile_targets_allow_new_authoring() {
        let mut content = inline_content();
        content.target.inline.as_mut().unwrap().image = None;
        content.reject_custom_image_selection().unwrap();

        content.target = serde_json::from_value(serde_json::json!({
            "sandbox_profile": {"id": "sbp_example", "revision": 1},
            "workspace_profile": {"id": "wkp_example", "revision": 1},
            "agent_profile": {"id": "agp_example", "revision": 1}
        }))
        .unwrap();
        content.validate().unwrap();
        content.reject_custom_image_selection().unwrap();
    }

    #[test]
    fn inline_target_rejects_mutable_image_reference() {
        let mut content = inline_content();
        content
            .target
            .inline
            .as_mut()
            .expect("inline target")
            .image
            .as_mut()
            .expect("image")
            .reference = Some("registry.example/agent:latest".into());
        let error = content
            .validate()
            .expect_err("mutable image must be rejected");
        assert_eq!(error.code, "invalid_image_reference");
        assert_eq!(error.field, "target.inline.image.reference");
    }
}

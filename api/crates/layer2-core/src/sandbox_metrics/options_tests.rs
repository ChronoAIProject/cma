use super::*;

fn query() -> OptionQuery {
    OptionQuery {
        kind: OptionKind::Owner,
        search: String::new(),
        owner: None,
        selected: None,
        cursor: None,
        limit: None,
    }
}
#[test]
fn option_inputs_are_bounded_and_cursor_domains_follow_search_and_owner() {
    let base = query();
    assert_eq!(base.validate().unwrap(), 50);
    for limit in [0, 51, u16::MAX] {
        assert!(OptionQuery {
            limit: Some(limit),
            ..base.clone()
        }
        .validate()
        .is_err());
    }
    for search in ["x".repeat(129), "a\nb".into()] {
        assert!(OptionQuery {
            search,
            ..base.clone()
        }
        .validate()
        .is_err());
    }
    assert!(OptionQuery {
        owner: Some("other".into()),
        ..base.clone()
    }
    .validate()
    .is_err());
    assert!(OptionQuery {
        selected: Some("one".into()),
        search: "two".into(),
        ..base.clone()
    }
    .validate()
    .is_err());
    let search = OptionQuery {
        search: "Alex".into(),
        ..base.clone()
    };
    assert_ne!(base.cursor_key().unwrap(), search.cursor_key().unwrap());
    let sandbox = OptionQuery {
        kind: OptionKind::Sandbox,
        owner: Some("owner".into()),
        ..base.clone()
    };
    assert!(sandbox.validate().is_ok());
    assert_ne!(
        sandbox.cursor_key().unwrap(),
        OptionQuery {
            owner: Some("other".into()),
            ..sandbox
        }
        .cursor_key()
        .unwrap()
    );
}
#[test]
fn labels_are_plain_and_never_fall_back_to_identifiers() {
    let id = "91892afa-5fa5-4f4f-9100-2b8d986d2f71";
    assert_eq!(
        option_label(OptionKind::Owner, id, Some("Alex Chen")),
        "Alex Chen"
    );
    for name in [
        None,
        Some("  "),
        Some(id),
        Some("User 91892afa-5fa5-4f4f-9100-2b8d986d2f71"),
    ] {
        assert_eq!(option_label(OptionKind::Owner, id, name), "Unnamed owner");
    }
    let sandbox = "sbx_00000000000000000000000000000001";
    assert_eq!(
        option_label(OptionKind::Sandbox, sandbox, Some(sandbox)),
        "Unnamed Sandbox"
    );
}

#[test]
fn duplicate_context_prefers_humans_and_never_changes_primary_labels() {
    let make = |id: &str, label: &str| FilterOption {
        value: id.into(),
        label: label.into(),
        disambiguator: None,
    };
    let mut items = vec![
        make("one", "Alex"),
        make("two", "Alex"),
        make("three", "Priya"),
    ];
    let hints = vec![
        option_joined(Some("2026-11-01T12:30:00Z")),
        option_joined(Some("2026-11-02T12:30:00Z")),
        Some("unused".into()),
    ];
    disambiguate_options(&mut items, &hints);
    assert_eq!(items[0].label, "Alex");
    assert_eq!(items[0].disambiguator.as_deref(), Some("Joined 2026-11-01"));
    assert_eq!(items[1].disambiguator.as_deref(), Some("Joined 2026-11-02"));
    assert!(items[2].disambiguator.is_none());
    disambiguate_options(
        &mut items,
        &[Some("same".into()), Some("same".into()), None],
    );
    assert_ne!(items[0].disambiguator, items[1].disambiguator);
    assert!(items[0]
        .disambiguator
        .as_deref()
        .unwrap()
        .starts_with("same · reference "));
    disambiguate_options(&mut items[..1], &hints[..1]);
    assert!(items[0].disambiguator.is_none());
    assert_eq!(items[0].label, "Alex");
    for at in [
        None,
        Some(""),
        Some("invalid"),
        Some("2026-99-99T00:00:00Z"),
        Some("9999-12-31T23:59:59-23:59"),
    ] {
        assert!(option_joined(at).is_none());
    }
}

#[test]
fn identifiers_attached_to_names_and_runtime_versions_are_never_labels() {
    let uuid = "91892afa-5fa5-4f4f-9100-2b8d986d2f71";
    let attached = format!("Research-{uuid}");
    for name in [uuid, attached.as_str()] {
        assert_eq!(
            option_label(OptionKind::Owner, "owner", Some(name)),
            "Unnamed owner"
        );
        assert_eq!(
            option_label(OptionKind::Sandbox, "sandbox", Some(name)),
            "Unnamed Sandbox"
        );
        assert_eq!(
            option_label(OptionKind::Runtime, name, None),
            "Unnamed runtime"
        );
    }
    assert_eq!(
        option_label(
            OptionKind::Runtime,
            "sbx_00000000000000000000000000000001",
            None
        ),
        "Unnamed runtime"
    );
    assert_eq!(
        option_label(OptionKind::Runtime, "runtime-stable", None),
        "runtime-stable"
    );
}

#[test]
fn runtime_pins_share_a_visible_digest_with_sandbox_details() {
    for (image, digest, expected) in [
        (
            "registry.example/team/runtime:stable",
            "a".repeat(64),
            "runtime:stable · sha256:aaaaaaaaaaaa",
        ),
        (
            "registry.example/team/runtime:stable",
            "b".repeat(64),
            "runtime:stable · sha256:bbbbbbbbbbbb",
        ),
        (
            "registry.example/owner/custom-runtime",
            "c".repeat(64),
            "custom-runtime · sha256:cccccccccccc",
        ),
        (
            "registry.example/owner/91892afa-5fa5-4f4f-9100-2b8d986d2f71",
            "d".repeat(64),
            "Runtime image · sha256:dddddddddddd",
        ),
    ] {
        let value = format!("{image}@sha256:{digest}");
        let label = option_label(OptionKind::Runtime, &value, None);
        assert_eq!(label, expected);
        let reference = label.rsplit_once(" · ").unwrap().1;
        assert!(value.contains(reference));
    }
}

fn contextual_pair(contexts: [OptionContext; 2]) -> Vec<FilterOption> {
    let mut items: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|value| FilterOption {
            value: value.into(),
            label: "Research".into(),
            disambiguator: None,
        })
        .collect();
    disambiguate_options_with_context(&mut items, &contexts);
    assert!(items.iter().all(|item| item.label == "Research"));
    items
}

fn unix(at: &str) -> u64 {
    time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339)
        .unwrap()
        .unix_timestamp() as u64
}

#[test]
fn same_owner_sandboxes_use_creation_date_then_minute_then_second() {
    for (other, expected) in [
        ("2026-11-02T12:30:00Z", ["2026-11-01", "2026-11-02"]),
        (
            "2026-11-01T12:31:00Z",
            ["2026-11-01 12:30 UTC", "2026-11-01 12:31 UTC"],
        ),
        (
            "2026-11-01T12:30:01Z",
            ["2026-11-01 12:30:00 UTC", "2026-11-01 12:30:01 UTC"],
        ),
    ] {
        let items = contextual_pair(
            ["2026-11-01T12:30:00Z", other]
                .map(|at| OptionContext::sandbox("alice-subject", Some("Alice"), Some(unix(at)))),
        );
        for (item, at) in items.iter().zip(expected) {
            assert_eq!(item.disambiguator, Some(format!("Alice · Created {at}")));
        }
    }
    let items = contextual_pair([
        OptionContext::sandbox("one", Some("Alice"), Some(unix("2026-11-01T12:30:00Z"))),
        OptionContext::sandbox("two", Some("Bob"), Some(unix("2026-11-02T12:30:00Z"))),
    ]);
    assert_eq!(items[0].disambiguator.as_deref(), Some("Alice"));
    assert_eq!(items[1].disambiguator.as_deref(), Some("Bob"));
}

#[test]
fn same_join_date_owners_use_utc_time_and_available_fractional_precision() {
    for (other, expected) in [
        ("2026-11-02T12:30:00Z", ["2026-11-01", "2026-11-02"]),
        (
            "2026-11-01T12:31:00Z",
            ["2026-11-01 12:30 UTC", "2026-11-01 12:31 UTC"],
        ),
        (
            "2026-11-01T12:30:01Z",
            ["2026-11-01 12:30:00 UTC", "2026-11-01 12:30:01 UTC"],
        ),
        (
            "2026-11-01T12:30:00.000000001Z",
            [
                "2026-11-01 12:30:00 UTC",
                "2026-11-01 12:30:00.000000001 UTC",
            ],
        ),
    ] {
        let items =
            contextual_pair(["2026-11-01T20:30:00+08:00", other].map(|at| precise_owner(Some(at))));
        for (item, at) in items.iter().zip(expected) {
            assert_eq!(item.disambiguator, Some(format!("Joined {at}")));
        }
    }
}

#[test]
fn creation_context_survives_missing_or_unsafe_owner_names() {
    let unsafe_name = "Alice-91892afa-5fa5-4f4f-9100-2b8d986d2f71";
    let items = contextual_pair([
        OptionContext::sandbox("one", Some(unsafe_name), Some(unix("2026-11-01T00:00:00Z"))),
        OptionContext::sandbox("two", None, Some(unix("2026-11-02T00:00:00Z"))),
    ]);
    assert_eq!(
        items[0].disambiguator.as_deref(),
        Some("Created 2026-11-01")
    );
    assert_eq!(
        items[1].disambiguator.as_deref(),
        Some("Created 2026-11-02")
    );
}

#[test]
fn references_are_last_resort_for_missing_invalid_or_identical_human_context() {
    for contexts in [
        [precise_owner(None), precise_owner(Some("invalid"))],
        [
            OptionContext::sandbox("one", None, None),
            OptionContext::sandbox("two", None, Some(u64::MAX)),
        ],
        [
            precise_owner(Some("2026-11-01T12:30:00.123Z")),
            precise_owner(Some("2026-11-01T20:30:00.123+08:00")),
        ],
        [
            OptionContext::sandbox("one", Some("Alice"), Some(1000)),
            OptionContext::sandbox("one", Some("Alice"), Some(1000)),
        ],
    ] {
        let mut items = contextual_pair(contexts.clone());
        assert_ne!(items[0].disambiguator, items[1].disambiguator);
        for item in &items {
            assert!(item
                .disambiguator
                .as_deref()
                .unwrap()
                .contains("reference "));
            assert!(!item.disambiguator.as_deref().unwrap().contains(&item.value));
        }
        let expected = items[0].disambiguator.clone();
        disambiguate_options_with_context(&mut items, &contexts);
        assert_eq!(items[0].disambiguator, expected);
        disambiguate_options_with_context(&mut items[..1], &contexts[..1]);
        assert!(items[0].disambiguator.is_none());
    }
}

#[test]
fn mixed_duplicate_group_only_escalates_context_for_remaining_ties() {
    let mut items: Vec<_> = (0..4)
        .map(|i| FilterOption {
            value: format!("item-{i}"),
            label: "Research".into(),
            disambiguator: None,
        })
        .collect();
    let contexts = [
        OptionContext::sandbox("alice", Some("Alice"), Some(1000)),
        OptionContext::sandbox("alice", Some("Alice"), Some(1001)),
        OptionContext::sandbox("bob", Some("Bob"), Some(1000)),
        OptionContext::sandbox("alice", Some("Alice"), Some(86400)),
    ];
    disambiguate_options_with_context(&mut items, &contexts);
    assert_eq!(
        items[0].disambiguator.as_deref(),
        Some("Alice · Created 1970-01-01 00:16:40 UTC")
    );
    assert_eq!(
        items[1].disambiguator.as_deref(),
        Some("Alice · Created 1970-01-01 00:16:41 UTC")
    );
    assert_eq!(items[2].disambiguator.as_deref(), Some("Bob"));
    assert_eq!(
        items[3].disambiguator.as_deref(),
        Some("Alice · Created 1970-01-02")
    );
}

fn precise_owner(at: Option<&str>) -> OptionContext {
    OptionContext::owner_with_access(at, OwnerContextAccess::DirectoryRead)
}

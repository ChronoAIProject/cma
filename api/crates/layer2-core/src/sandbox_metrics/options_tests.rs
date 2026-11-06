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

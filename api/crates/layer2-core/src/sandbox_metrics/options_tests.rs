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
fn labels_distinguish_duplicates_and_never_fall_back_to_identifiers() {
    let id = "91892afa-5fa5-4f4f-9100-2b8d986d2f71";
    let first = option_label(OptionKind::Owner, id, Some("Alex Chen"));
    assert!(first.starts_with("Alex Chen · account "));
    assert!(!first.contains(id));
    assert_ne!(
        first,
        option_label(OptionKind::Owner, "another", Some("Alex Chen"))
    );
    assert_eq!(
        first,
        option_label(OptionKind::Owner, id, Some("Alex Chen"))
    );
    for name in [
        None,
        Some("  "),
        Some(id),
        Some("User 91892afa-5fa5-4f4f-9100-2b8d986d2f71"),
    ] {
        assert!(option_label(OptionKind::Owner, id, name).starts_with("Unnamed owner · account "));
    }
    let sandbox = "sbx_00000000000000000000000000000001";
    assert!(option_label(OptionKind::Sandbox, sandbox, Some(sandbox))
        .starts_with("Unnamed Sandbox · Sandbox "));
    assert!(!option_label(OptionKind::Sandbox, sandbox, None).contains(sandbox));
}

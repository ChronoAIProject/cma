use super::*;
#[test]
fn rolling_usage_view_keys_are_stable_and_scope_complete() {
    let at = 1_800_000_000;
    let a = ViewInput::default().validate(at).unwrap();
    let b = ViewInput::default().validate(at + 600).unwrap();
    let scope = Scope {
        principal: "owner".into(),
        admin: true,
    };
    assert_ne!(a.query.range(), b.query.range());
    assert_eq!(a.key(&scope).unwrap(), b.key(&scope).unwrap());
    assert_ne!(
        a.key(&scope).unwrap(),
        a.key(&Scope {
            admin: false,
            ..scope.clone()
        })
        .unwrap()
    );
    assert_ne!(
        a.key(&scope).unwrap(),
        a.key(&Scope {
            principal: "other".into(),
            ..scope.clone()
        })
        .unwrap()
    );
    let week = ViewInput {
        window: ViewWindow::Week,
        ..Default::default()
    }
    .validate(at + 60)
    .unwrap();
    assert_eq!(week.query.step_seconds, 600);
    assert_ne!(week.key(&scope).unwrap(), a.key(&scope).unwrap());
    let mut filtered = ViewInput::default();
    filtered.filters.statuses = vec![Status::Paused, Status::Active, Status::Paused];
    filtered.filters.search = "  HELLO ".into();
    let normalized = filtered.clone().validate(at).unwrap();
    filtered.filters.statuses = vec![Status::Active, Status::Paused];
    filtered.filters.search = "hello".into();
    assert_eq!(
        normalized.key(&scope).unwrap(),
        filtered.validate(at).unwrap().key(&scope).unwrap()
    );
}
#[test]
fn custom_view_ranges_are_exact_and_rolling_ranges_reject_absolute_overrides() {
    let at = 1_800_000_000;
    assert!(ViewInput {
        from: Some(at - 600),
        ..Default::default()
    }
    .validate(at)
    .is_err());
    assert!(ViewInput {
        window: ViewWindow::Custom,
        ..Default::default()
    }
    .validate(at)
    .is_err());
    let a = ViewInput {
        window: ViewWindow::Custom,
        from: Some(at - 600),
        to: Some(at),
        ..Default::default()
    }
    .validate(at)
    .unwrap();
    let b = ViewInput {
        window: ViewWindow::Custom,
        from: Some(at - 660),
        to: Some(at),
        ..Default::default()
    }
    .validate(at)
    .unwrap();
    let scope = Scope {
        principal: "owner".into(),
        admin: false,
    };
    assert_ne!(a.key(&scope).unwrap(), b.key(&scope).unwrap());
}

#[test]
fn preset_defaults_fit_bucket_limit_at_every_minute_alignment() {
    let base = 1_800_000_000;
    for minute in 0..60 {
        let at = base + minute * 60;
        for (window, step) in [
            (ViewWindow::Day, 60),
            (ViewWindow::Week, 600),
            (ViewWindow::Month, 3600),
        ] {
            let selected = ViewInput {
                window,
                ..Default::default()
            }
            .validate(at)
            .unwrap();
            assert_eq!(selected.query.step_seconds, step);
            let buckets = (selected.query.to - 1) / step - selected.query.from / step + 1;
            assert!(buckets <= 2016, "{window:?} minute {minute}: {buckets}");
        }
        // An explicit client resolution keeps its strict contract. Omitting
        // the preset override is what allows the stable server default.
        let explicit = ViewInput {
            window: ViewWindow::Week,
            step_seconds: Some(300),
            ..Default::default()
        }
        .validate(at);
        assert_eq!(explicit.is_ok(), at % 300 == 0);
    }
}

use super::*;
fn parse(entries: &[(&str, &str)]) -> Result<ViewPolicy, String> {
    ViewPolicy::from_lookup(&mut |key| {
        Ok(entries
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.to_string()))
    })
}
#[test]
fn retained_usage_views_and_warmer_require_separate_explicit_activation() {
    let policy = parse(&[]).unwrap();
    assert!(!policy.enabled && !policy.warm_enabled && policy.warm_scopes.is_empty());
    assert_eq!(
        (
            policy.freshness_seconds,
            policy.retain_seconds,
            policy.warm_seconds
        ),
        (300, 86_400, 300)
    );
    assert!(parse(&[("CHRONO_USAGE_REPORT_WARMER_ENABLED", "true")]).is_err());
    assert!(parse(&[
        ("CHRONO_USAGE_REPORT_VIEWS_ENABLED", "true"),
        ("CHRONO_USAGE_REPORT_WARMER_ENABLED", "true")
    ])
    .is_err());
    let cohort = r#"[{"principal":"owner","admin":false,"sandbox_id":"sbx_00000000000000000000000000000001"}]"#;
    let policy = parse(&[
        ("CHRONO_USAGE_REPORT_VIEWS_ENABLED", "true"),
        ("CHRONO_USAGE_REPORT_WARMER_ENABLED", "true"),
        ("CHRONO_USAGE_REPORT_VIEW_SCOPES", cohort),
        ("CHRONO_USAGE_REPORT_WARM_SCOPES", cohort),
    ])
    .unwrap();
    assert_eq!(policy.warm_scopes.len(), 1);
    for entries in [
        vec![("CHRONO_USAGE_REPORT_VIEWS_ENABLED", "yes")],
        vec![("CHRONO_USAGE_REPORT_RETAIN_SECONDS", "86401")],
        vec![("CHRONO_USAGE_REPORT_WARM_SECONDS", "0")],
        vec![(
            "CHRONO_USAGE_REPORT_WARM_SCOPES",
            r#"[{"principal":"owner","admin":true,"token":"forbidden-field"}]"#,
        )],
    ] {
        assert!(parse(&entries).is_err());
    }
    assert!(ViewPolicy::from_lookup(&mut |_| Err("lookup failed".into())).is_err());
}

#[test]
fn warmer_windows_are_explicit_bounded_and_keep_legacy_default() {
    let views = r#"[{"principal":"owner","admin":true,"sandbox_id":null}]"#;
    let parse_warm = |warm| {
        parse(&[
            ("CHRONO_USAGE_REPORT_VIEWS_ENABLED", "true"),
            ("CHRONO_USAGE_REPORT_WARMER_ENABLED", "true"),
            ("CHRONO_USAGE_REPORT_VIEW_SCOPES", views),
            ("CHRONO_USAGE_REPORT_WARM_SCOPES", warm),
        ])
    };
    assert_eq!(
        parse_warm(views).unwrap().warm_scopes[0].window,
        ViewWindow::Day
    );
    let warm = r#"[{"principal":"owner","admin":true,"sandbox_id":null,"window":"24h"},{"principal":"owner","admin":true,"sandbox_id":null,"window":"7d"},{"principal":"owner","admin":true,"sandbox_id":null,"window":"30d"}]"#;
    let policy = parse_warm(warm).unwrap();
    assert_eq!(
        policy
            .warm_scopes
            .iter()
            .map(|s| s.window)
            .collect::<Vec<_>>(),
        vec![ViewWindow::Day, ViewWindow::Week, ViewWindow::Month]
    );
    for invalid in [
        warm.replace("30d", "custom"),
        warm.replace("30d", "7d"),
        warm.replace("30d", "31d"),
        warm.replace("owner", "foreign"),
        warm.replace("\"window\"", "\"windows\""),
    ] {
        assert!(parse_warm(&invalid).is_err());
    }
    assert!(parse(&[("CHRONO_USAGE_REPORT_VIEW_SCOPES", warm)]).is_err());
}

#[test]
fn incremental_allocation_projection_requires_explicit_activation() {
    assert!(!parse(&[]).unwrap().allocation_rollups_enabled);
    assert!(parse(&[("CHRONO_USAGE_ALLOCATION_ROLLUPS_ENABLED","true")]).unwrap().allocation_rollups_enabled);
    for value in ["yes", "1", "TRUE", ""] {
        assert!(parse(&[("CHRONO_USAGE_ALLOCATION_ROLLUPS_ENABLED",value)]).is_err());
    }
}

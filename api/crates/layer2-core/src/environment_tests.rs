//! Tests for the environment profile model.
//!
//! Kept in a sibling file so `environment.rs` stays within the project's
//! file-size limit while the behaviour remains covered.

use super::*;

fn spec(name: &str) -> EnvironmentProfileSpec {
    EnvironmentProfileSpec {
        name: name.to_string(),
        ..EnvironmentProfileSpec::default()
    }
}

fn baseline() -> Baseline {
    Baseline::new(
        [SoftwarePackage::new("platform-cli")],
        [SkillId::new("mandatory-one"), SkillId::new("mandatory-two")],
    )
}

#[test]
fn a_variable_never_renders_its_value() {
    // The value is exactly where a caller will put a token, so `{:?}` must not
    // be a route to it. The key stays visible: it is what makes a log useful.
    let v = EnvironmentVariable::new("API_TOKEN", "super-secret-value");
    let rendered = format!("{v:?}");
    assert!(!rendered.contains("super-secret-value"), "{rendered}");
    assert!(rendered.contains("API_TOKEN"), "{rendered}");
    assert!(rendered.contains("redacted"), "{rendered}");
}

#[test]
fn variable_names_follow_the_posix_shape() {
    for good in ["PATH", "_HIDDEN", "a1", "MY_VAR_2"] {
        assert!(EnvironmentVariable::is_valid_key(good), "{good} is usable");
    }
    for bad in [
        "",
        "1STARTS_WITH_DIGIT",
        "HAS-DASH",
        "HAS SPACE",
        "HAS=EQUALS",
    ] {
        assert!(
            !EnvironmentVariable::is_valid_key(bad),
            "{bad:?} must be rejected"
        );
    }
}

#[test]
fn a_named_profile_with_nothing_in_it_is_valid() {
    assert!(spec("empty").validate().is_ok());
}

#[test]
fn an_unnamed_profile_is_rejected() {
    // Whitespace counts as empty: a profile called "   " is unusable in a
    // picker and would be indistinguishable from every other one.
    for name in ["", "   "] {
        let err = spec(name).validate().expect_err("must be rejected");
        assert!(matches!(err, CoreError::InvalidRequest(_)), "got {err:?}");
    }
}

#[test]
fn an_overlong_name_is_rejected() {
    let err = spec(&"x".repeat(MAX_NAME + 1))
        .validate()
        .expect_err("must be rejected");
    assert_eq!(err.code(), "invalid_request");
}

#[test]
fn a_malformed_variable_name_is_rejected() {
    let mut s = spec("bad");
    s.variables.push(EnvironmentVariable::new("NOT VALID", "x"));
    let err = s.validate().expect_err("must be rejected");
    assert!(err.to_string().contains("NOT VALID"), "{err}");
}

#[test]
fn the_same_variable_set_twice_is_rejected() {
    // A session cannot hold one name twice, so accepting this would mean
    // silently choosing which of the two wins.
    let mut s = spec("dupes");
    s.variables.push(EnvironmentVariable::new("TOKEN", "a"));
    s.variables.push(EnvironmentVariable::new("TOKEN", "b"));
    let err = s.validate().expect_err("must be rejected");
    assert!(err.to_string().contains("TOKEN"), "{err}");
}

#[test]
fn a_rejected_value_is_never_echoed_back() {
    let mut s = spec("huge");
    s.variables
        .push(EnvironmentVariable::new("BIG", "s".repeat(MAX_VALUE + 1)));
    let err = s.validate().expect_err("must be rejected");
    let message = err.to_string();
    assert!(message.contains("BIG"), "{message}");
    assert!(
        !message.contains("ssss"),
        "an error must not quote the value: {message}"
    );
}

#[test]
fn oversized_lists_are_rejected_per_list() {
    let mut variables = spec("many");
    variables.variables = (0..=MAX_ENTRIES)
        .map(|i| EnvironmentVariable::new(format!("V{i}"), "x"))
        .collect();
    assert!(variables.validate().is_err());

    let mut packages = spec("many");
    packages.packages = (0..=MAX_ENTRIES)
        .map(|i| SoftwarePackage::new(format!("p{i}")))
        .collect();
    assert!(packages.validate().is_err());

    let mut skills = spec("many");
    skills.skills = (0..=MAX_ENTRIES)
        .map(|i| SkillId::new(format!("s{i}")))
        .collect();
    assert!(skills.validate().is_err());
}

#[test]
fn blank_packages_and_skills_are_rejected() {
    let mut packages = spec("blank");
    packages.packages.push(SoftwarePackage::new("  "));
    assert!(packages.validate().is_err());

    let mut skills = spec("blank");
    skills.skills.push(SkillId::new(""));
    assert!(skills.validate().is_err());
}

#[test]
fn a_package_renders_its_pin_when_it_has_one() {
    assert_eq!(SoftwarePackage::new("ripgrep").requirement(), "ripgrep");
    assert_eq!(
        SoftwarePackage {
            name: "ripgrep".into(),
            version: Some("14".into()),
        }
        .requirement(),
        "ripgrep@14"
    );
}

#[test]
fn the_baseline_is_added_to_a_caller_who_omitted_it() {
    // The guarantee in one test: asking for nothing still yields the platform's
    // mandatory entries.
    let mut s = spec("mine");
    baseline().apply(&mut s);
    assert_eq!(
        s.packages
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        vec!["platform-cli"]
    );
    assert_eq!(
        s.skills.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        vec!["mandatory-one", "mandatory-two"]
    );
}

#[test]
fn the_baseline_cannot_be_dropped_by_listing_everything_else() {
    // The removal a client would actually attempt: send the list back without
    // the entries it does not want. They come straight back.
    let mut s = spec("mine");
    s.packages.push(SoftwarePackage::new("ripgrep"));
    s.skills.push(SkillId::new("chosen"));
    baseline().apply(&mut s);

    assert!(s.packages.iter().any(|p| p.name == "platform-cli"));
    assert!(s.skills.contains(&SkillId::new("mandatory-one")));
    assert!(s.skills.contains(&SkillId::new("mandatory-two")));
    // What the caller did choose survives alongside it.
    assert!(s.packages.iter().any(|p| p.name == "ripgrep"));
    assert!(s.skills.contains(&SkillId::new("chosen")));
}

#[test]
fn applying_the_baseline_twice_does_not_duplicate_it() {
    // It is applied on write and again on resolve, so idempotence is what stops
    // a profile accumulating a copy of the baseline per edit.
    let mut s = spec("mine");
    baseline().apply(&mut s);
    baseline().apply(&mut s);
    assert_eq!(s.packages.len(), 1);
    assert_eq!(s.skills.len(), 2);
}

#[test]
fn a_caller_may_pin_a_mandatory_package_without_getting_it_twice() {
    // Pinning a version of a mandatory package is a legitimate choice; adding
    // the unpinned copy alongside would install it twice.
    let mut s = spec("pinned");
    s.packages.push(SoftwarePackage {
        name: "platform-cli".into(),
        version: Some("2.1".into()),
    });
    baseline().apply(&mut s);
    assert_eq!(s.packages.len(), 1);
    assert_eq!(s.packages[0].requirement(), "platform-cli@2.1");
}

#[test]
fn the_baseline_reports_what_it_mandates() {
    let b = baseline();
    assert!(b.requires_package("platform-cli"));
    assert!(!b.requires_package("ripgrep"));
    assert!(b.requires_skill(&SkillId::new("mandatory-one")));
    assert!(!b.requires_skill(&SkillId::new("something-else")));
}

#[test]
fn an_empty_baseline_mandates_nothing() {
    let b = Baseline::default();
    let mut s = spec("mine");
    b.apply(&mut s);
    assert!(s.packages.is_empty());
    assert!(s.skills.is_empty());
    assert!(!b.requires_package("anything"));
}

#[test]
fn selecting_no_profile_still_yields_the_baseline() {
    let resolved = ResolvedEnvironment::baseline_only(&baseline());
    assert!(resolved.variables.is_empty());
    assert_eq!(resolved.packages, vec!["platform-cli".to_string()]);
    assert_eq!(
        resolved.skills,
        vec!["mandatory-one".to_string(), "mandatory-two".to_string()]
    );
}

#[test]
fn a_resolved_profile_carries_its_variables_by_name() {
    let profile = EnvironmentProfile {
        id: EnvironmentProfileId::generate(),
        owner: SubjectId("subject-a".into()),
        name: "mine".into(),
        variables: vec![EnvironmentVariable::new("TOKEN", "value")],
        packages: vec![SoftwarePackage::new("ripgrep")],
        skills: vec![SkillId::new("chosen")],
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    };
    let resolved = ResolvedEnvironment::from_spec(&profile.to_spec());
    assert_eq!(
        resolved.variables.get("TOKEN").map(String::as_str),
        Some("value")
    );
    assert_eq!(resolved.packages, vec!["ripgrep".to_string()]);

    // The adapter layer receives the same bundle without reinterpreting it.
    let handed: layer3_adapter::RuntimeEnvironment = resolved.into();
    assert_eq!(
        handed.variables.get("TOKEN").map(String::as_str),
        Some("value")
    );
    assert_eq!(handed.skills, vec!["chosen".to_string()]);
}

#[test]
fn ownership_is_exact_match_only() {
    let profile = EnvironmentProfile {
        id: EnvironmentProfileId::generate(),
        owner: SubjectId("subject-a".into()),
        name: "mine".into(),
        variables: Vec::new(),
        packages: Vec::new(),
        skills: Vec::new(),
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    };
    assert!(profile.is_owned_by(&SubjectId("subject-a".into())));
    assert!(!profile.is_owned_by(&SubjectId("subject-b".into())));
}

#[test]
fn catalogue_errors_carry_their_context() {
    let e = CatalogError::Unavailable("connection refused".into());
    assert!(e.to_string().contains("connection refused"));
    let e = CatalogError::Malformed("not json".into());
    assert!(e.to_string().contains("not json"));
}

use super::home::from_lookup;

const VALUES: [(&str, &str); 4] = [
    ("CHRONO_SANDBOX_HOME_STORAGE_CLASS", "cma-home-v1"),
    ("CHRONO_SANDBOX_HOME_CAPACITY_GIB", "64"),
    ("CHRONO_SANDBOX_HOME_TEMPLATE", "cma-home-seed-v1"),
    ("CHRONO_SANDBOX_HOME_TEMPLATE_UID", "original-seed-uid"),
];

#[test]
fn only_absent_policy_keeps_legacy_homes_and_partial_policy_fails_closed() {
    assert!(from_lookup(&mut |_| Ok(None)).unwrap().is_none());
    for (absent, (missing_key, _)) in VALUES.iter().enumerate() {
        let error = from_lookup(&mut |key| {
            Ok(VALUES
                .iter()
                .enumerate()
                .find(|(index, (name, _))| *index != absent && *name == key)
                .map(|(_, (_, value))| value.to_string()))
        })
        .unwrap_err();
        assert!(error.contains(missing_key));
    }
    let policy = from_lookup(&mut |key| {
        Ok(VALUES
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.to_string()))
    })
    .unwrap()
    .unwrap();
    assert_eq!(policy.capacity_gib, 64);
    assert_eq!(policy.template_generation, "original-seed-uid");
}

#[test]
fn storage_policy_refuses_ambiguous_or_out_of_bounds_values() {
    for (key, _) in VALUES {
        let invalid = if key.ends_with("GIB") {
            vec!["", "0", "1025", "064", "+64", "64Gi", " 64"]
        } else {
            vec![
                "",
                " ",
                "-seed",
                "seed-",
                "other/seed",
                "Uppercase",
                "seed\n",
            ]
        };
        for value in invalid {
            assert!(
                from_lookup(&mut |name| Ok(VALUES
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(k, default)| if *k == key { value } else { default }.to_string())))
                .is_err(),
                "{key}={value:?}"
            );
        }
    }
}

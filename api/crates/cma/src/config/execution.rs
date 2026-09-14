use layer2_core::sandbox::ProvisioningExecution;

use super::{parse_u64, value_or};

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<ProvisioningExecution, String> {
    let command_timeout_seconds = parse_u64(
        "CHRONO_INSTALL_COMMAND_TIMEOUT_SECONDS",
        &value_or(lookup, "CHRONO_INSTALL_COMMAND_TIMEOUT_SECONDS", "600")?,
        1,
        600,
    )?;
    let total_timeout_seconds = parse_u64(
        "CHRONO_INSTALL_TOTAL_TIMEOUT_SECONDS",
        &value_or(lookup, "CHRONO_INSTALL_TOTAL_TIMEOUT_SECONDS", "1800")?,
        command_timeout_seconds,
        1_800,
    )?;
    let clone_depth = parse_u64(
        "CHRONO_CLONE_DEPTH",
        &value_or(lookup, "CHRONO_CLONE_DEPTH", "50")?,
        1,
        1_000,
    )? as u32;
    Ok(ProvisioningExecution {
        command_timeout_seconds,
        total_timeout_seconds,
        clone_depth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides_are_bounded() {
        assert_eq!(
            from_lookup(&mut |_| Ok(None)).unwrap(),
            ProvisioningExecution::default()
        );
        let custom = from_lookup(&mut |key| {
            Ok(Some(
                match key {
                    "CHRONO_INSTALL_COMMAND_TIMEOUT_SECONDS" => "20",
                    "CHRONO_INSTALL_TOTAL_TIMEOUT_SECONDS" => "30",
                    "CHRONO_CLONE_DEPTH" => "2",
                    _ => unreachable!(),
                }
                .into(),
            ))
        })
        .unwrap();
        assert_eq!(custom.command_timeout_seconds, 20);
        assert_eq!(custom.total_timeout_seconds, 30);
        assert_eq!(custom.clone_depth, 2);
        for (key, value) in [
            ("CHRONO_INSTALL_COMMAND_TIMEOUT_SECONDS", "0"),
            ("CHRONO_INSTALL_COMMAND_TIMEOUT_SECONDS", "601"),
            ("CHRONO_INSTALL_TOTAL_TIMEOUT_SECONDS", "599"),
            ("CHRONO_INSTALL_TOTAL_TIMEOUT_SECONDS", "1801"),
            ("CHRONO_CLONE_DEPTH", "0"),
            ("CHRONO_CLONE_DEPTH", "1001"),
            ("CHRONO_CLONE_DEPTH", "abc"),
        ] {
            let error =
                from_lookup(&mut |found| Ok((found == key).then(|| value.into()))).unwrap_err();
            assert!(error.contains(key), "{error}");
        }
        assert_eq!(
            from_lookup(&mut |_| Err("non-Unicode".into())).unwrap_err(),
            "non-Unicode"
        );
    }
}

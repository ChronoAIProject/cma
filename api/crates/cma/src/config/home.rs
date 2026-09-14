//! An optional, complete storage policy. Partial configuration must fail startup.

use layer3_adapter::HomeVolumePolicy;

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Option<HomeVolumePolicy>, String> {
    let storage = lookup("CHRONO_SANDBOX_HOME_STORAGE_CLASS")?;
    let capacity = lookup("CHRONO_SANDBOX_HOME_CAPACITY_GIB")?;
    let template = lookup("CHRONO_SANDBOX_HOME_TEMPLATE")?;
    let generation = lookup("CHRONO_SANDBOX_HOME_TEMPLATE_UID")?;
    if [&storage, &capacity, &template, &generation]
        .iter()
        .all(|value| value.is_none())
    {
        return Ok(None);
    }
    let required = |value: Option<String>, key: &str| {
        value
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{key} is required when durable Sandbox homes are configured"))
    };
    let capacity = required(capacity, "CHRONO_SANDBOX_HOME_CAPACITY_GIB")?;
    let policy = HomeVolumePolicy {
        storage_class: required(storage, "CHRONO_SANDBOX_HOME_STORAGE_CLASS")?,
        capacity_gib: capacity
            .parse::<u32>()
            .ok()
            .filter(|value| (1..=1024).contains(value) && value.to_string() == capacity)
            .ok_or("CHRONO_SANDBOX_HOME_CAPACITY_GIB must be a whole GiB value from 1 to 1024")?,
        template_name: required(template, "CHRONO_SANDBOX_HOME_TEMPLATE")?,
        template_generation: required(generation, "CHRONO_SANDBOX_HOME_TEMPLATE_UID")?,
    };
    for (name, value) in [
        ("CHRONO_SANDBOX_HOME_STORAGE_CLASS", &policy.storage_class),
        ("CHRONO_SANDBOX_HOME_TEMPLATE", &policy.template_name),
        (
            "CHRONO_SANDBOX_HOME_TEMPLATE_UID",
            &policy.template_generation,
        ),
    ] {
        if value.is_empty()
            || value.len() > 253
            || !value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || value.starts_with('-')
            || value.ends_with('-')
        {
            return Err(format!("{name} must be a lowercase DNS identifier"));
        }
    }
    policy.validate().map_err(|error| error.to_string())?;
    Ok(Some(policy))
}

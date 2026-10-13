//! An optional, complete storage policy. Partial configuration must fail startup.

use layer3_adapter::HomeVolumePolicy;

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Option<HomeVolumePolicy>, String> {
    read_policy(
        lookup,
        false,
        [
            "CHRONO_SANDBOX_HOME_STORAGE_CLASS",
            "CHRONO_SANDBOX_HOME_CAPACITY_GIB",
            "CHRONO_SANDBOX_HOME_TEMPLATE",
            "CHRONO_SANDBOX_HOME_TEMPLATE_UID",
        ],
    )
}

pub(super) fn native_from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    fallback: Option<&HomeVolumePolicy>,
) -> Result<Option<HomeVolumePolicy>, String> {
    Ok(read_policy(
        lookup,
        true,
        [
            "CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS",
            "CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB",
            "CHRONO_AGENT_SANDBOX_HOME_TEMPLATE",
            "CHRONO_AGENT_SANDBOX_HOME_TEMPLATE_UID",
        ],
    )?
    .or_else(|| fallback.cloned()))
}

fn read_policy(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    empty_as_absent: bool,
    keys: [&str; 4],
) -> Result<Option<HomeVolumePolicy>, String> {
    let configured = |value: &String| !empty_as_absent || !value.is_empty();
    let storage = lookup(keys[0])?.filter(configured);
    let capacity = lookup(keys[1])?.filter(configured);
    let template = lookup(keys[2])?.filter(configured);
    let generation = lookup(keys[3])?.filter(configured);
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
    let capacity = required(capacity, keys[1])?;
    let policy = HomeVolumePolicy {
        storage_class: required(storage, keys[0])?,
        capacity_gib: capacity
            .parse::<u32>()
            .ok()
            .filter(|value| (1..=1024).contains(value) && value.to_string() == capacity)
            .ok_or_else(|| format!("{} must be a whole GiB value from 1 to 1024", keys[1]))?,
        template_name: required(template, keys[2])?,
        template_generation: required(generation, keys[3])?,
    };
    for (name, value) in [
        (keys[0], &policy.storage_class),
        (keys[2], &policy.template_name),
        (keys[3], &policy.template_generation),
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

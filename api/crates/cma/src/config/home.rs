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
    let keys = [
        "CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS",
        "CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB",
        "CHRONO_AGENT_SANDBOX_HOME_TEMPLATE",
        "CHRONO_AGENT_SANDBOX_HOME_TEMPLATE_UID",
    ];
    let values = keys
        .iter()
        .map(|key| lookup(key).map(|value| value.filter(|v| !v.is_empty())))
        .collect::<Result<Vec<_>, _>>()?;
    let mode = lookup("CHRONO_AGENT_SANDBOX_HOME_MODE")?.filter(|v| !v.is_empty());
    if mode.as_deref().is_some_and(|v| {
        ![
            "native_vct_empty_v1",
            "native_vct_full_home_v2",
            "legacy_snapshot",
        ]
        .contains(&v)
    }) {
        return Err(
            "CHRONO_AGENT_SANDBOX_HOME_MODE must be native_vct_full_home_v2, native_vct_empty_v1 or legacy_snapshot".into(),
        );
    }
    if values.iter().all(Option::is_none) && mode.is_none() {
        return Ok(fallback.cloned());
    }
    let empty = matches!(
        mode.as_deref(),
        Some("native_vct_empty_v1" | "native_vct_full_home_v2")
    ) || (mode.is_none() && values[2].is_none() && values[3].is_none());
    if empty {
        if values[2].is_some() || values[3].is_some() {
            return Err("native VCT empty homes cannot specify a snapshot template".into());
        }
        let storage = values[0]
            .clone()
            .or_else(|| {
                mode.as_ref()
                    .and_then(|_| fallback.map(|p| p.storage_class.clone()))
            })
            .ok_or_else(|| format!("{} is required for native VCT homes", keys[0]))?;
        let capacity = values[1]
            .clone()
            .or_else(|| {
                mode.as_ref()
                    .and_then(|_| fallback.map(|p| p.capacity_gib.to_string()))
            })
            .ok_or_else(|| format!("{} is required for native VCT homes", keys[1]))?;
        let capacity_gib = capacity
            .parse::<u32>()
            .ok()
            .filter(|v| (1..=1024).contains(v) && v.to_string() == capacity)
            .ok_or_else(|| format!("{} must be a whole GiB value from 1 to 1024", keys[1]))?;
        if storage.len() > 253
            || storage.is_empty()
            || storage.starts_with('-')
            || storage.ends_with('-')
            || !storage
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(format!("{} must be a lowercase DNS identifier", keys[0]));
        }
        return Ok(Some(HomeVolumePolicy {
            mode: if mode.as_deref() == Some("native_vct_empty_v1") {
                layer3_adapter::HomeVolumeMode::NativeVctEmptyV1
            } else {
                layer3_adapter::HomeVolumeMode::NativeVctFullHomeV2
            },
            storage_class: storage,
            capacity_gib,
            template_name: String::new(),
            template_generation: String::new(),
        }));
    }
    let mut saved = |key: &str| {
        Ok(keys
            .iter()
            .position(|k| *k == key)
            .and_then(|i| values[i].clone()))
    };
    Ok(read_policy(&mut saved, true, keys)?.or_else(|| fallback.cloned()))
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
        mode: Default::default(),
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

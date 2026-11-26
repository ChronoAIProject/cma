use super::FeatureFlags;

pub(super) fn validate(flags: &FeatureFlags) -> Result<(), String> {
    if flags.usage_snapshot_cursors_enabled && !flags.usage_signed_cursors_enabled {
        return Err(
            "CHRONO_USAGE_SNAPSHOT_CURSORS_ENABLED requires CHRONO_USAGE_SIGNED_CURSORS_ENABLED"
                .into(),
        );
    }
    if flags.usage_strict_cursors_enabled && !flags.usage_signed_cursors_enabled {
        return Err(
            "CHRONO_USAGE_STRICT_CURSORS_ENABLED requires CHRONO_USAGE_SIGNED_CURSORS_ENABLED"
                .into(),
        );
    }
    Ok(())
}

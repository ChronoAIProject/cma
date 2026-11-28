declare const __CHRONO_WEB_REPLICA_RECOVERY_ENABLED__: boolean;

// The validated build input enables general replica recovery. Browser storage,
// URLs and runtime configuration cannot change a deployed bundle's policy.
export const replicaRecoveryEnabled = __CHRONO_WEB_REPLICA_RECOVERY_ENABLED__;

declare const __CHRONO_WEB_INPUT_REFUSAL_RECOVERY_ENABLED__: boolean;

export const inputRefusalRecoveryEnabled = __CHRONO_WEB_INPUT_REFUSAL_RECOVERY_ENABLED__;

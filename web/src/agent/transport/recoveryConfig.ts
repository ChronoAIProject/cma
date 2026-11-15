declare const __CHRONO_WEB_REPLICA_RECOVERY_ENABLED__: boolean;

// Only the validated build input can enable automatic mutations. No browser
// storage, URL or runtime configuration may change a deployed bundle's policy.
export const replicaRecoveryEnabled = __CHRONO_WEB_REPLICA_RECOVERY_ENABLED__;

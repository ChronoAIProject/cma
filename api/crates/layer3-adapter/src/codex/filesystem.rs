//! Codex-specific credential custody added at the trusted adapter boundary.
pub(super) fn protect_provider_credentials(owner_access: bool, protected_paths: &mut Vec<String>) {
    if !owner_access {
        protected_paths.extend([".codex/auth.json".into(), "cma/.codex/auth.json".into()]);
    }
}

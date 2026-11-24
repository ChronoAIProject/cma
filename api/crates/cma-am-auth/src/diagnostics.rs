//! Stable diagnostic targets for authentication events.

// Preserve the auth emitter's original module target and filter namespace.
pub const AUTHENTICATION_TARGET: &str = "cma::auth::middleware::failure_diagnostics";
pub const PROVIDER_IDENTITY_TARGET: &str = "cma::auth::verification_diagnostics";
pub const ORGANIZATION_PRINCIPAL_TARGET: &str = "cma::auth::organizations::principal_diagnostics";

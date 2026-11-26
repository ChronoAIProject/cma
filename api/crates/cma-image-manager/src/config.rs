//! Image Manager deployment configuration contract.
pub const CMA_IM_MONGODB_URI: &str = "CMA_IM_MONGODB_URI";
pub const CMA_IM_MONGODB_DATABASE: &str = "CMA_IM_MONGODB_DATABASE";
pub const CMA_IM_MONGODB_AUTHORITY_MODE: &str = "CMA_IM_MONGODB_AUTHORITY_MODE";
pub const CMA_IM_NYXID_ISSUER: &str = "CMA_IM_NYXID_ISSUER";
pub const CMA_IM_NYXID_CLIENT_ID: &str = "CMA_IM_NYXID_CLIENT_ID";
pub const CMA_IM_NYXID_CLIENT_SECRET: &str = "CMA_IM_NYXID_CLIENT_SECRET";
pub const CMA_IM_COOKIE_KEY_FILE: &str = "CMA_IM_COOKIE_KEY_FILE";
pub const CMA_IM_WEB_ORIGIN: &str = "CMA_IM_WEB_ORIGIN";
pub const CMA_IM_LISTEN: &str = "CMA_IM_LISTEN";
pub const CMA_IM_BUILD_SERVICE_ACCOUNT: &str = "CMA_IM_BUILD_SERVICE_ACCOUNT";
// Required server settings; no example destination is safe as a runtime default.
pub const CMA_IM_BUILD_PROJECT: &str = "CMA_IM_BUILD_PROJECT";
pub const CMA_IM_BUILD_LOCATION: &str = "CMA_IM_BUILD_LOCATION";
// Retained build references are relative to this repository. Preserve the
// original value across releases; moving retained images requires a migration.
pub const CMA_IM_IMAGE_REGISTRY: &str = "CMA_IM_IMAGE_REGISTRY";
pub const CMA_IM_BUILDER_IMAGE: &str = "CMA_IM_BUILDER_IMAGE";
pub const CMA_IM_GOOGLE_TOKEN_FILE: &str = "CMA_IM_GOOGLE_TOKEN_FILE";

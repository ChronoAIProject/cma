//! Service-owned role catalog. NyxID holds assignments and issues effective grants.
use serde::Serialize;
#[derive(Serialize)]
pub struct Role {
    pub name: &'static str,
    pub permissions: &'static [&'static str],
}
pub const ROLES: &[Role] = &[
    Role {
        name: "cma-eg-viewer",
        permissions: &["cma-eg:channels:read"],
    },
    Role {
        name: "cma-eg-manager",
        permissions: &[
            "cma-eg:channels:read",
            "cma-eg:channels:create",
            "cma-eg:channels:update",
            "cma-eg:channels:delete",
            "cma-eg:channels:test",
        ],
    },
    Role {
        name: "cma-eg-admin",
        permissions: &["cma-eg:admin"],
    },
];

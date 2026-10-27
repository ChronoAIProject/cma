//! Permission names belong to Image Manager. Legacy CMA/Agent Manager
//! wildcards and an unscoped role named `admin` do not grant image access.

use serde::Serialize;

pub const READ: &str = "cma-im:images:read";
pub const CREATE: &str = "cma-im:images:create";
pub const CONTROL: &str = "cma-im:images:update";
pub const DELETE: &str = "cma-im:images:delete";
pub const SHARE: &str = "cma-im:images:share";
pub const USE: &str = "cma-im:images:use";
pub const BUILD: &str = "cma-im:builds:create";
pub const BUILD_READ: &str = "cma-im:builds:read";
pub const ADMIN: &str = "cma-im:admin";

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Role {
    pub slug: &'static str,
    pub permissions: &'static [&'static str],
}

pub const ROLES: &[Role] = &[
    Role {
        slug: "cma-im-viewer",
        permissions: &[READ, USE, BUILD_READ],
    },
    Role {
        slug: "cma-im-user",
        permissions: &[READ, CREATE, CONTROL, DELETE, SHARE, USE, BUILD, BUILD_READ],
    },
    Role {
        slug: "cma-im-admin",
        permissions: &[
            READ, CREATE, CONTROL, DELETE, SHARE, USE, BUILD, BUILD_READ, ADMIN,
        ],
    },
];

pub fn allows(permissions: &[String], required: &str) -> bool {
    [
        READ, CREATE, CONTROL, DELETE, SHARE, USE, BUILD, BUILD_READ, ADMIN,
    ]
    .contains(&required)
        && permissions.iter().any(|p| p == required || p == ADMIN)
}

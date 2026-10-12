//! Permission names belong to Sandbox Manager. Legacy CMA/Agent Manager
//! wildcards and an unscoped role named `admin` do not grant sandbox access.

use serde::Serialize;

pub const READ: &str = "cma-sm:sandboxes:read";
pub const CREATE: &str = "cma-sm:sandboxes:create";
pub const CONTROL: &str = "cma-sm:sandboxes:control";
pub const DELETE: &str = "cma-sm:sandboxes:delete";
pub const ADMIN: &str = "cma-sm:admin";

#[derive(Debug, Serialize)]
pub struct Role {
    pub slug: &'static str,
    pub permissions: &'static [&'static str],
}

pub const ROLES: &[Role] = &[
    Role {
        slug: "cma-sm-viewer",
        permissions: &[READ],
    },
    Role {
        slug: "cma-sm-user",
        permissions: &[READ, CREATE, CONTROL, DELETE],
    },
    Role {
        slug: "cma-sm-admin",
        permissions: &[READ, CREATE, CONTROL, DELETE, ADMIN],
    },
];

pub fn allows(permissions: &[String], required: &str) -> bool {
    [READ, CREATE, CONTROL, DELETE, ADMIN].contains(&required)
        && permissions.iter().any(|p| p == required || p == ADMIN)
}

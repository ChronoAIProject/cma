//! Every console route and the authority it requires. The router is built
//! from this table, so a route cannot exist without an entry, and a test
//! walks it: only the sign-in, login, callback, signed-out and logout routes
//! may be public; every other route, read or write, takes the one
//! `Authorized` extractor and derives authority on every request.
use axum::routing::{get, post, MethodRouter};

use super::{channel_routes, login, pages::channel, redirect, routes, Console};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Sign-in plumbing: never reads or writes channel data.
    Public,
    /// Takes `Authorized`, whether it reads or writes.
    Authorized,
}

#[derive(Debug, Clone, Copy)]
pub struct ConsoleRoute {
    pub method: &'static str,
    pub path: &'static str,
    pub access: Access,
}

pub const ROUTES: &[ConsoleRoute] = &[
    ConsoleRoute {
        method: "GET",
        path: redirect::HOME,
        access: Access::Authorized,
    },
    ConsoleRoute {
        method: "GET",
        path: redirect::LOGIN,
        access: Access::Public,
    },
    ConsoleRoute {
        method: "GET",
        path: redirect::CALLBACK,
        access: Access::Public,
    },
    ConsoleRoute {
        method: "GET",
        path: redirect::SIGNED_OUT,
        access: Access::Public,
    },
    ConsoleRoute {
        method: "POST",
        path: redirect::LOGOUT,
        access: Access::Public,
    },
    ConsoleRoute {
        method: "POST",
        path: redirect::OWNER,
        access: Access::Authorized,
    },
    ConsoleRoute {
        method: "GET",
        path: channel::PATH,
        access: Access::Authorized,
    },
];

/// The handler of `route`. A table entry without a handler fails the first
/// router build, which every console test performs.
pub(crate) fn handler(route: &ConsoleRoute) -> MethodRouter<Console> {
    match (route.method, route.path) {
        ("GET", redirect::HOME) => get(routes::home),
        ("GET", redirect::LOGIN) => get(login::start),
        ("GET", redirect::CALLBACK) => get(login::callback),
        ("GET", redirect::SIGNED_OUT) => get(routes::signed_out),
        ("POST", redirect::LOGOUT) => post(routes::logout),
        ("POST", redirect::OWNER) => post(routes::switch_owner),
        ("GET", channel::PATH) => get(channel_routes::channel),
        (method, path) => unreachable!("console route {method} {path} has no handler"),
    }
}

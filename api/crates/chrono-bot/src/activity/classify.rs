//! Turning a request into the core's vocabulary.
//!
//! This file is the whole translation: the core deals in actions, targets and
//! allowlisted parameters, and knows nothing about how a caller reached it.
//! Everything protocol-shaped stops here.
//!
//! **Nothing is recorded that is not named below.** The body is not copied,
//! summarised or searched — it is parsed into a struct that has a field for
//! each parameter somebody decided may be kept, and serde discards the rest.
//! That is what makes this safe as the request shape grows: a field added to a
//! request later has nowhere to land. A denylist would have to be updated in
//! step with every such field, and would leak the first time it was not.
//!
//! Headers, cookies and credentials are never read here at all. There is no
//! code path that could record one, which is stronger than a rule that says
//! not to.

use axum::http::Method;
use layer2_core::{
    ActivityAction as Action, ActivityDetail, ActivityTarget, DetailKey, EnvironmentProfileId,
    SessionId,
};
use serde::Deserialize;

/// What a request is asking for, in the core's terms.
pub struct Classified {
    /// The action being taken.
    pub action: Action,
    /// What it is being taken on, when it names one thing.
    pub target: Option<ActivityTarget>,
    /// Whether the body carries parameters worth keeping.
    ///
    /// Read so the body is only buffered when there is something in it to
    /// record: every other request is passed straight through untouched.
    pub reads_body: bool,
}

impl Classified {
    fn new(action: Action) -> Self {
        Self {
            action,
            target: None,
            reads_body: false,
        }
    }

    fn on(mut self, target: ActivityTarget) -> Self {
        self.target = Some(target);
        self
    }

    fn with_body(mut self) -> Self {
        self.reads_body = true;
        self
    }
}

/// What the service was asked for, or `None` if this request asked for nothing
/// the service serves.
///
/// Returning `None` is deliberate for three kinds of request. A liveness probe
/// arrives every few seconds, carries no identity and asks for nothing — a
/// record of them would bury every entry that matters. A description of the
/// service names no resource and no subject. And a path this service does not
/// serve did not do anything, whoever sent it.
pub fn classify(method: &Method, path: &str) -> Option<Classified> {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let session = |id: &str| ActivityTarget::Session(SessionId(id.to_string()));
    let profile =
        |id: &str| ActivityTarget::EnvironmentProfile(EnvironmentProfileId(id.to_string()));

    match segments.as_slice() {
        // Signing in is recorded by the flow that completes it, which is the
        // only thing that knows who signed in: the request that starts one
        // carries no identity yet, and attributing it to nobody would leave
        // the entry that matters unattributed.
        ["auth", "logout"] => Some(Classified::new(Action::SignOut)),

        ["api", "v1", "sessions"] => match *method {
            Method::GET => Some(Classified::new(Action::SessionList)),
            Method::POST => Some(Classified::new(Action::SessionCreate).with_body()),
            _ => None,
        },
        ["api", "v1", "sessions", id] => match *method {
            Method::GET => Some(Classified::new(Action::SessionRead).on(session(id))),
            Method::DELETE => Some(Classified::new(Action::SessionDelete).on(session(id))),
            _ => None,
        },
        ["api", "v1", "sessions", id, verb] => classify_session_verb(method, verb).map(|action| {
            let classified = Classified::new(action).on(session(id));
            // The one session verb whose body says something worth keeping:
            // which plane. Everything else a caller sends to a session is
            // their own content — see below.
            match action {
                Action::SessionPlaneChange => classified.with_body(),
                _ => classified,
            }
        }),
        // The request identifier is in the path rather than the body, so it is
        // recorded without reading one — and the decision, which is in the
        // body, deliberately is not.
        ["api", "v1", "sessions", id, "approvals", _request] if method == Method::POST => {
            Some(Classified::new(Action::SessionApprovalResolve).on(session(id)))
        }

        ["api", "v1", "environment-profiles"] => match *method {
            Method::GET => Some(Classified::new(Action::EnvironmentProfileList)),
            Method::POST => Some(Classified::new(Action::EnvironmentProfileCreate).with_body()),
            _ => None,
        },
        // Matched ahead of the identifier pattern below, exactly as the router
        // matches the static segment ahead of it.
        ["api", "v1", "environment-profiles", "skills"] => {
            Some(Classified::new(Action::SkillCatalogRead))
        }
        ["api", "v1", "environment-profiles", id] => match *method {
            Method::GET => Some(Classified::new(Action::EnvironmentProfileRead).on(profile(id))),
            Method::PUT => Some(
                Classified::new(Action::EnvironmentProfileReplace)
                    .on(profile(id))
                    .with_body(),
            ),
            Method::DELETE => {
                Some(Classified::new(Action::EnvironmentProfileDelete).on(profile(id)))
            }
            _ => None,
        },

        ["api", "v1", "admin", "users"] => Some(Classified::new(Action::UserDirectoryRead)),
        ["api", "v1", "admin", "activity"] => Some(Classified::new(Action::ActivityLogRead)),
        ["api", "v1", "admin", "runtimes"] => Some(Classified::new(Action::RuntimeInventoryRead)),
        ["api", "v1", "admin", "sessions"] => Some(Classified::new(Action::SessionList)),
        ["api", "v1", "admin", "settings"] => match *method {
            Method::GET => Some(Classified::new(Action::PlatformSettingsRead)),
            Method::PATCH => Some(Classified::new(Action::PlatformSettingsChange)),
            _ => None,
        },
        ["api", "v1", "admin", "settings", "reset"] if method == Method::POST => {
            Some(Classified::new(Action::PlatformSettingsReset))
        }
        ["api", "v1", "admin", "sessions", id, verb] => classify_session_verb(method, verb)
            .map(|action| Classified::new(action).on(session(id))),

        _ => None,
    }
}

/// The operations addressed as a verb under one session.
fn classify_session_verb(method: &Method, verb: &str) -> Option<Action> {
    match (method, verb) {
        (&Method::POST, "terminate") => Some(Action::SessionTerminate),
        (&Method::POST, "stop") => Some(Action::SessionStop),
        (&Method::POST, "revive") => Some(Action::SessionRevive),
        (&Method::POST, "plane") => Some(Action::SessionPlaneChange),
        (&Method::GET, "screen") => Some(Action::SessionScreenRead),
        (&Method::GET, "transcript") => Some(Action::SessionTranscriptRead),
        (&Method::GET, "events") => Some(Action::SessionEventsRead),
        (&Method::GET, "terminal") => Some(Action::SessionTerminalAttach),
        // Asking for a ticket is asking to attach, recorded as the same
        // action: what happened is that somebody set out to open this
        // terminal, and splitting that into two entries would say twice what
        // happened once.
        //
        // No body is read and no query is named. A ticket's value must never
        // reach a record, and the way that is guaranteed is that there is no
        // field here for one to land in — `detail_from_query` names three keys
        // and `ticket` is not among them.
        (&Method::POST, "terminal-ticket") => Some(Action::SessionTerminalAttach),
        // The operating verbs. None of them reads a body, and that is the
        // point rather than an omission: what a caller sends to a session is
        // keystrokes or a prompt, and what they answer an approval with is a
        // decision about their own work. Both are the owner's content. There
        // is no field here for either to land in, which is a stronger
        // guarantee than a rule saying not to record them.
        (&Method::POST, "input") => Some(Action::SessionInput),
        (&Method::POST, "interrupt") => Some(Action::SessionInterrupt),
        (&Method::POST, "steer") => Some(Action::SessionSteer),
        (&Method::POST, "resize") => Some(Action::SessionResize),
        (&Method::GET, "usage") => Some(Action::SessionUsageRead),
        _ => None,
    }
}

/// The one field of a plane-change request that may be kept.
///
/// Which plane, and nothing else. The same allowlist rule as everything above:
/// a field added to this request later has nowhere here to land.
#[derive(Debug, Deserialize)]
struct SessionPlaneBody {
    #[serde(default)]
    plane: Option<String>,
}

/// The fields of a session-creation request that may be kept.
///
/// This struct is the allowlist for that body. Serde discards every field it
/// does not name, so nothing else can be recorded however the request grows.
#[derive(Debug, Deserialize)]
struct SessionCreateBody {
    #[serde(default)]
    kernel: Option<String>,
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
    #[serde(default)]
    environment_profile: Option<String>,
    #[serde(default)]
    options: Option<SessionOptionsBody>,
}

/// The agent choices worth keeping. Not the credential mode, which says whose
/// account is being spent and is nobody's business but the owner's.
#[derive(Debug, Deserialize)]
struct SessionOptionsBody {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    permission: Option<String>,
}

/// The fields of an environment-bundle request that may be kept.
///
/// The three lists are read as [`serde::de::IgnoredAny`], which is the point:
/// the type can hold a count and cannot hold a value. A bundle's variables are
/// where a caller is invited to put a credential, and there is no version of
/// this struct that has one in it to leak.
#[derive(Debug, Deserialize)]
struct ProfileBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    variables: Vec<serde::de::IgnoredAny>,
    #[serde(default)]
    packages: Vec<serde::de::IgnoredAny>,
    #[serde(default)]
    skills: Vec<serde::de::IgnoredAny>,
}

/// Read the parameters worth keeping out of a request body.
///
/// A body that cannot be parsed records nothing rather than failing: the
/// request itself is about to be refused by the handler for the same reason,
/// and the entry saying so is worth more than no entry at all.
pub fn detail_from_body(action: Action, body: &[u8]) -> ActivityDetail {
    let mut detail = ActivityDetail::new();
    match action {
        Action::SessionCreate => {
            let Ok(parsed) = serde_json::from_slice::<SessionCreateBody>(body) else {
                return detail;
            };
            if let Some(kernel) = parsed.kernel {
                detail.record(DetailKey::Runtime, kernel);
            }
            if let Some(cols) = parsed.cols {
                detail.record(DetailKey::Columns, cols.to_string());
            }
            if let Some(rows) = parsed.rows {
                detail.record(DetailKey::Rows, rows.to_string());
            }
            if let Some(id) = parsed.environment_profile {
                detail.record(DetailKey::EnvironmentProfile, id);
            }
            if let Some(options) = parsed.options {
                if let Some(model) = options.model {
                    detail.record(DetailKey::Model, model);
                }
                if let Some(permission) = options.permission {
                    detail.record(DetailKey::PermissionMode, permission);
                }
            }
        }
        Action::SessionPlaneChange => {
            let Ok(parsed) = serde_json::from_slice::<SessionPlaneBody>(body) else {
                return detail;
            };
            if let Some(plane) = parsed.plane {
                detail.record(DetailKey::Plane, plane);
            }
        }
        Action::EnvironmentProfileCreate | Action::EnvironmentProfileReplace => {
            let Ok(parsed) = serde_json::from_slice::<ProfileBody>(body) else {
                return detail;
            };
            if let Some(name) = parsed.name {
                detail.record(DetailKey::ResourceName, name);
            }
            detail.record(DetailKey::VariableCount, parsed.variables.len().to_string());
            detail.record(DetailKey::PackageCount, parsed.packages.len().to_string());
            detail.record(DetailKey::SkillCount, parsed.skills.len().to_string());
        }
        // Every other action carries nothing in its body worth keeping, and is
        // never buffered in the first place.
        _ => {}
    }
    detail
}

/// Read the parameters worth keeping out of a query string.
///
/// Three keys, matched by name, and the two numeric ones only when they really
/// are numbers. Anything else in the query is ignored, so a parameter added
/// later cannot arrive in the record by accident.
///
/// Values are kept as the caller wrote them, escaping and all. Decoding them
/// would mean this file interpreting a caller's text, and an identifier that
/// reads back slightly oddly is a smaller problem than one that was decoded
/// into something it never said.
pub fn detail_from_query(query: Option<&str>, detail: &mut ActivityDetail) {
    let Some(query) = query else {
        return;
    };
    for (key, value) in query.split('&').filter_map(|p| p.split_once('=')) {
        match key {
            "after" if value.parse::<u64>().is_ok() => detail.record(DetailKey::Cursor, value),
            "limit" if value.parse::<u64>().is_ok() => detail.record(DetailKey::Limit, value),
            // Which subject a reader narrowed the record to. An identifier, and
            // the one thing worth knowing about a filtered read.
            "actor" if !value.is_empty() => detail.record(DetailKey::Subject, value),
            // Everything else is ignored, and one thing in particular: the
            // terminal upgrade carries a `ticket`, which is a credential for
            // the seconds it lives. There is no arm for it, which is stronger
            // than a rule saying not to add one.
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "classify_tests.rs"]
mod tests;

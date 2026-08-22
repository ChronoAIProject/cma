//! Settings that belong to the deployment rather than to anyone using it.
//!
//! Everything else this core models is owned by a subject: a session, a
//! profile, a credential. These are owned by whoever runs the service, apply
//! to every session created afterwards, and are changed by an explicit act of
//! an administrator rather than by anybody's ordinary use.
//!
//! The module is deliberately a *set* of settings rather than a succession of
//! special cases. Each setting remains independently named and typed as the set
//! grows.

use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

// Re-exported so the split is a move rather than a rename: callers still read
// deployment policy through `platform`, while the range has a file named for
// the type and does not consume this settings file as more settings arrive.
pub use crate::capacity_bounds::{CapacityBounds, OutOfBounds, SizeDimension};

/// What a session is told about itself before its agent does anything.
///
/// **Assembled, never a free-form document.** The section naming what the
/// session may reach is a field here rather than a marker inside prose, which
/// makes a document that omits it unrepresentable: there is no value of this
/// type, and therefore no stored record and no request body, that leaves a
/// session unable to learn what it can touch.
///
/// That is the whole reason for the shape. The alternative — one editable
/// string containing a placeholder — can be saved with the placeholder
/// deleted, duplicated or misspelled, and then needs a save-time refusal and a
/// render-time fallback to recover from a hazard the design itself introduced.
///
/// Everything an administrator *can* change here is something that cannot
/// break a session: the prose around the list, the words the list is
/// introduced by, and where the list sits. Whether the list exists is not
/// theirs, and neither is what is in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBriefing {
    /// Prose placed above the reach section, in order.
    ///
    /// May be empty. An administrator who empties it removes what the platform
    /// says about the session being authenticated, and that is permitted
    /// deliberately: prose the platform cannot correct is worse than prose an
    /// administrator can get wrong. The surface offering it says what it costs
    /// and offers the shipped text back in one action.
    #[serde(default = "SessionBriefing::default_before")]
    pub before: Vec<String>,
    /// What the reach section is called.
    ///
    /// Editable, because naming a section cannot break it. Blank falls back to
    /// the shipped heading rather than producing an unlabelled list.
    #[serde(default = "SessionBriefing::default_reach_heading")]
    pub reach_heading: String,
    /// The sentences introducing the list, before the entries themselves.
    #[serde(default = "SessionBriefing::default_reach_lead")]
    pub reach_lead: String,
    /// The sentences closing the reach section, after the entries.
    #[serde(default = "SessionBriefing::default_reach_close")]
    pub reach_close: String,
    /// What is said instead when a session was granted nothing.
    ///
    /// Its own field rather than a special case of the lead, because it is a
    /// different sentence with a different job: the list version explains how
    /// to use what is there, and this one explains that there is nothing.
    #[serde(default = "SessionBriefing::default_reach_none")]
    pub reach_none: String,
    /// Prose placed below the reach section, in order.
    #[serde(default)]
    pub after: Vec<String>,
}

/// Every setting this deployment holds.
///
/// One field per setting, so a client built before a setting existed cannot
/// erase it and a client inventing one is refused rather than silently
/// ignored. The struct grows; nothing here is a map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlatformSettings {
    #[serde(default)]
    pub briefing: SessionBriefing,
    /// What a session on this deployment may ask for.
    ///
    /// Admin-editable at runtime rather than configuration, because a bound is
    /// policy an operator changes while the service runs and a config key would
    /// cost a rollout and create a second source of truth.
    ///
    /// Tightening a bound is not retroactive. It refuses new asks and stops
    /// nothing already running. These bounds apply only to a size a session
    /// states; an unnamed size uses the deployment's own reservation, which the
    /// bounds do not govern.
    ///
    /// Named `size_bounds` rather than `capacity`: a later deployment-wide
    /// ledger answers whether aggregate space exists, which is a different
    /// setting and question from this per-request range.
    #[serde(default)]
    pub size_bounds: CapacityBounds,
    /// How many sessions one subject may hold live at once.
    ///
    /// `None` is no limit, and is what the platform ships: a deployment that
    /// has decided nothing must not start refusing sessions when this setting
    /// first appears.
    ///
    /// Lowering this is not retroactive. It stops new asks and never stops
    /// running work, including during reconciliation after a control-plane
    /// restart. An owner already over the limit who stops a session cannot
    /// revive it until fewer than the limit are live.
    ///
    /// This is per subject. The core has no tenant, organization, or account
    /// grouping above `SubjectId`, so two subjects receive two limits.
    #[serde(default)]
    pub live_sessions: Option<NonZeroU32>,
}

impl Default for SessionBriefing {
    /// What the platform ships, and what a reset restores.
    ///
    /// Lives here rather than in the adapter that renders it so a deployment
    /// which has never been configured behaves exactly as one that has been
    /// reset — the same words, from the same place, rather than two texts that
    /// can drift.
    ///
    /// Vendor names appear in these strings and that does not breach §3. The
    /// rule is that no protocol or kernel name may appear in this core's
    /// *identifiers, types or comments* — what the core must not do is know
    /// what they mean or branch on them. These are opaque prose, carried and
    /// never read, exactly as an environment variable's value is.
    fn default() -> Self {
        Self {
            before: vec![
                "# This session".into(),
                "You are already authenticated. A credential was minted for this session alone and is\n\
                 attached to your requests automatically. You will never see its value and do not need it."
                    .into(),
                "**Do not run `nyxid login`.** There is nobody to log in as. The same goes for\n\
                 `nyxid whoami`, `nyxid status` and `nyxid service list`: they ask about a user account,\n\
                 this session is not one, and they will fail however many times they are tried."
                    .into(),
                "Reach a service with `nyxid proxy request <slug> <path>`.".into(),
            ],
            reach_heading: "## What you can reach".into(),
            reach_lead: "These slugs and no others. `nyxid proxy discover` lists everything on the account\n\
                         rather than what this session was granted, so treat this list as the truth and that\n\
                         one as a catalogue:"
                .into(),
            reach_close: "A call to anything else is refused for the life of this session. It is not worth\n\
                          retrying."
                .into(),
            reach_none: "This session was granted no services, so every proxied call will be refused. That\n\
                         is a property of how it was created and will not change while it runs."
                .into(),
            after: Vec::new(),
        }
    }
}

impl SessionBriefing {
    fn default_before() -> Vec<String> {
        Self::default().before
    }

    fn default_reach_heading() -> String {
        Self::default().reach_heading
    }

    fn default_reach_lead() -> String {
        Self::default().reach_lead
    }

    fn default_reach_close() -> String {
        Self::default().reach_close
    }

    fn default_reach_none() -> String {
        Self::default().reach_none
    }

    /// Render the document a session is handed, given what it may reach.
    ///
    /// The reach list is the caller's and never the setting's: it is a fact
    /// about one session, decided when that session was created, and an
    /// administrator editing prose must not be able to change it.
    pub fn render(&self, reaches: &[String]) -> String {
        let mut out = String::new();
        for block in &self.before {
            push_block(&mut out, block);
        }

        if reaches.is_empty() {
            push_block(&mut out, &self.reach_none);
        } else {
            let heading = match self.reach_heading.trim() {
                // An unlabelled list reads as a continuation of whatever came
                // before it, so a blank heading falls back rather than omitting.
                "" => Self::default().reach_heading,
                named => named.to_string(),
            };
            push_block(&mut out, &heading);
            push_block(&mut out, &self.reach_lead);
            for slug in reaches {
                out.push_str(&format!("- `{slug}`\n"));
            }
            out.push('\n');
            push_block(&mut out, &self.reach_close);
        }

        for block in &self.after {
            push_block(&mut out, block);
        }
        // Exactly one newline at the end, never the blank line `push_block`
        // leaves behind. What follows this document is the owner's own half,
        // appended by the adapter behind its own rule, and a stray blank line
        // here would put two where the composer this replaces put one.
        out.truncate(out.trim_end().len());
        out.push('\n');
        out
    }
}

/// Append one block, separated by a blank line, skipping empties.
///
/// An empty block contributes nothing rather than a run of blank lines: an
/// administrator clearing a field should get a shorter document, not a ragged
/// one.
fn push_block(out: &mut String, block: &str) {
    let block = block.trim();
    if block.is_empty() {
        return;
    }
    out.push_str(block);
    out.push_str("\n\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four slugs, the shape a real session carries.
    fn reaches() -> Vec<String> {
        ["ornn", "chrono-llm", "ornn-api", "api-github-pat"]
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn the_shipped_default_renders_what_the_service_already_writes() {
        /*
         * The fixture is not a transcription of the design. It is the file
         * captured from a running production pod — `/home/chrono/AGENTS.md` on
         * a session created on 2026-08-21 — so this asserts that making the
         * document configurable changed nothing for a deployment that
         * configures nothing.
         *
         * Byte-for-byte deliberately. "Looks the same" is how a blank line
         * moves and nobody notices until an agent reads it differently.
         */
        // `.txt`, not `.md`: it is a fixture rather than documentation, and
        // §2 puts all documentation under `docs/`.
        let shipped = include_str!("platform_fixture_briefing.txt");
        assert_eq!(
            SessionBriefing::default().render(&reaches()),
            shipped,
            "the default must render exactly what the hardcoded composer wrote"
        );
    }

    #[test]
    fn a_session_that_reaches_nothing_is_told_so_rather_than_shown_an_empty_list() {
        let out = SessionBriefing::default().render(&[]);
        assert!(out.contains("granted no services"));
        assert!(
            !out.contains("## What you can reach"),
            "a heading over nothing is worse than no heading: {out}"
        );
    }

    #[test]
    fn an_administrator_cannot_produce_a_document_without_the_reach_section() {
        /*
         * The property the whole shape exists for.
         *
         * Everything an administrator can set is set to nothing here — every
         * prose block emptied, the heading blanked — and the session is still
         * told what it may reach. There is no value of this type that omits
         * it, which is why this is a type and not a string with a placeholder
         * in it.
         */
        let stripped = SessionBriefing {
            before: Vec::new(),
            reach_heading: String::new(),
            reach_lead: String::new(),
            reach_close: String::new(),
            reach_none: String::new(),
            after: Vec::new(),
        };
        let out = stripped.render(&reaches());
        for slug in reaches() {
            assert!(
                out.contains(&format!("- `{slug}`")),
                "every granted slug survives an administrator emptying everything: {out}"
            );
        }
        // And the list is still labelled, because an unlabelled one reads as a
        // continuation of whatever came before it.
        assert!(out.contains("## What you can reach"), "{out}");
    }

    #[test]
    fn the_reach_list_is_the_sessions_and_never_the_settings() {
        // An administrator writing slugs into their prose does not grant them.
        // What a session may touch is decided when it is created.
        let meddling = SessionBriefing {
            before: vec!["You can reach `everything` and `anything`.".into()],
            ..SessionBriefing::default()
        };
        let out = meddling.render(&["ornn".to_string()]);
        assert!(out.contains("- `ornn`"));
        assert!(
            !out.contains("- `everything`"),
            "prose is prose; only the session's own grant becomes the list: {out}"
        );
    }

    #[test]
    fn emptying_a_block_shortens_the_document_rather_than_leaving_a_gap() {
        // An administrator clearing a field should get a shorter document, not
        // a ragged one with a run of blank lines where it used to be.
        let out = SessionBriefing {
            before: vec![
                "# Kept".into(),
                String::new(),
                "   ".into(),
                "Also kept".into(),
            ],
            ..SessionBriefing::default()
        }
        .render(&reaches());
        assert!(!out.contains("\n\n\n"), "no run of blank lines: {out:?}");
        assert!(out.starts_with("# Kept\n\nAlso kept\n\n"), "{out:?}");
    }

    #[test]
    fn an_administrator_may_remove_the_platforms_own_prose() {
        /*
         * Permitted deliberately, and the product owner decided it: prose the
         * platform cannot correct is worse than prose an administrator can get
         * wrong. The surface that offers this states what it costs and offers
         * the shipped text back in one action.
         *
         * Pinned so that the day somebody makes it impossible, they do it on
         * purpose and change this test rather than discovering it in review.
         */
        let out = SessionBriefing {
            before: Vec::new(),
            ..SessionBriefing::default()
        }
        .render(&reaches());
        assert!(!out.contains("already authenticated"));
        assert!(
            out.contains("- `ornn`"),
            "and the session still knows its reach"
        );
    }

    #[test]
    fn a_settings_document_written_before_a_field_existed_still_reads() {
        let empty: PlatformSettings = serde_json::from_str("{}").expect("old empty settings read");
        assert_eq!(empty, PlatformSettings::default());

        let partial: PlatformSettings =
            serde_json::from_str(r##"{"briefing":{"reach_heading":"# Different words"}}"##)
                .expect("a partially written briefing reads");
        assert_eq!(
            partial,
            PlatformSettings {
                briefing: SessionBriefing {
                    reach_heading: "# Different words".into(),
                    ..SessionBriefing::default()
                },
                ..PlatformSettings::default()
            }
        );
    }
}

//! Per-event source lookups. A channel may hold several chat sources, so
//! code acting on one activity or message finds that activity's own source
//! instead of the first source of a kind.
use super::{ChannelDefinition, Extensions, NativeLarkSource, Source};
use crate::activity::SourceKind;

impl ChannelDefinition {
    /// The source an activity arrived on: Telegram{bot_id} → the native source with that
    /// bot_id; NyxidRelay{platform,..} → the relay with that platform; Custom and Github → their source.
    pub fn source_of(&self, kind: &SourceKind) -> Option<&Source> {
        self.sources.iter().find(|source| match (kind, source) {
            (SourceKind::NativeLark(identity), Source::NativeLark(source)) => {
                identity == &source.identity
            }
            (
                SourceKind::Whatsapp {
                    app_id,
                    waba_id,
                    phone_number_id,
                },
                Source::Whatsapp {
                    app_id: app,
                    waba_id: account,
                    phone_number_id: phone,
                    ..
                },
            ) => app_id == app && waba_id == account && phone_number_id == phone,
            (
                SourceKind::Telegram { bot_id, .. },
                Source::Telegram {
                    bot_id: configured, ..
                },
            ) => bot_id == configured,
            (
                SourceKind::NyxidRelay { platform, .. },
                Source::NyxidRelay {
                    platform: configured,
                    ..
                },
            ) => platform == configured,
            (
                SourceKind::Discord { bot_id },
                Source::Discord {
                    bot_id: configured, ..
                },
            ) => bot_id == configured,
            (
                SourceKind::Github {
                    app_ref,
                    installation_id,
                    repository_id,
                },
                Source::Github {
                    app_ref: configured,
                    installation_ids,
                    repository_ids,
                    deny_repository_ids,
                    ..
                },
            ) => {
                app_ref == configured
                    && installation_ids.contains(installation_id)
                    && repository_ids.contains(repository_id)
                    && !deny_repository_ids.contains(repository_id)
            }
            (SourceKind::Custom, Source::Custom { .. }) => true,
            _ => false,
        })
    }

    /// The relay whose route_ids contain `route_id` (unique by the disjointness rule).
    pub fn relay_source_for_route(&self, route_id: &str) -> Option<&Source> {
        self.sources.iter().find(|source| {
            matches!(source, Source::NyxidRelay { route_ids, .. }
                if route_ids.iter().any(|route| route == route_id))
        })
    }

    /// The bot username of the source an activity arrived on.
    pub fn bot_username_of(&self, kind: &SourceKind) -> Option<&str> {
        match self.source_of(kind)? {
            Source::Telegram { bot_username, .. } | Source::NyxidRelay { bot_username, .. } => {
                bot_username.as_deref()
            }
            Source::NativeLark(_)
            | Source::Whatsapp { .. }
            | Source::Custom { .. }
            | Source::Discord { .. }
            | Source::Github { .. } => None,
        }
    }

    /// Whether the definition needs the extended readers. Code with a
    /// legacy, first-of-a-kind behaviour keeps it exactly while this is false.
    pub fn has_extended_sources(&self) -> bool {
        Extensions::of(self).sources
    }
}

impl Source {
    pub fn native_bot_mentions(&self) -> bool {
        match self {
            Self::NativeLark(NativeLarkSource { admission, .. })
            | Self::Whatsapp { admission, .. }
            | Self::Telegram { admission, .. }
            | Self::Discord { admission, .. }
            | Self::NyxidRelay { admission, .. } => admission.native_bot_mentions(),
            Self::Custom { .. } | Self::Github { .. } => false,
        }
    }

    pub fn github_accepts(&self, event: &str, action: Option<&str>) -> bool {
        let Self::Github {
            events, actions, ..
        } = self
        else {
            return false;
        };
        events.iter().any(|candidate| candidate == event)
            && action.is_some_and(|value| actions.iter().any(|candidate| candidate == value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_source_requires_pinned_facts_and_allowlisted_action() {
        let source = Source::Github {
            app_ref: "chrono".into(),
            installation_ids: vec![7],
            repository_ids: vec![42],
            deny_repository_ids: vec![],
            events: vec!["issues".into()],
            actions: vec!["opened".into()],
            prompt_template: None,
        };
        assert!(source.github_accepts("issues", Some("opened")));
        assert!(!source.github_accepts("issues", Some("closed")));
        assert!(!source.github_accepts("pull_request", Some("opened")));
        assert_eq!(source.source_id().as_str(), "github");
    }
}

//! The notices above the channel pages: how complete the owner index is, and
//! which operator flags limit what a channel does or what the console
//! offers. All text is fixed console text.
use crate::console::source_changes::SourceAvailability;

/// "Loading channels… n" while the first listing pages through CMA, and the
/// partial-index explanation once the index reached its bound.
pub(crate) fn index(loading: Option<u64>, partial: bool) -> String {
    let mut html = String::new();
    if let Some(seen) = loading {
        html.push_str(&format!(
            "<p class=\"notice\" role=\"status\">Loading channels… {seen}</p>"
        ));
    }
    if partial {
        html.push_str("<p class=\"notice\" role=\"status\">Partial index: the gateway holds at most its configured number of channels in memory, so some channels may be missing from this list. Each one still opens from its own address.</p>");
    }
    html
}

/// Why source controls are absent, if they are.
pub(crate) fn sources(availability: SourceAvailability) -> String {
    let text = match availability {
        SourceAvailability::Available => return String::new(),
        SourceAvailability::Disabled => "Source changes are disabled by the operator.",
        SourceAvailability::NotYetAvailable => {
            "Source changes are not available in this gateway release yet."
        }
    };
    format!("<p class=\"notice\" role=\"status\">{text}</p>")
}

/// What a routed channel does while the operator has multi-consumer routing
/// off; nothing for a legacy channel, or while routing is on.
pub(crate) fn routing(routed: bool, default_receives_all: bool, multi_consumer: bool) -> String {
    if !routed || multi_consumer {
        return String::new();
    }
    let text = if default_receives_all {
        "Multi-consumer routing is disabled by the operator. All events continue to the default consumer."
    } else {
        "Multi-consumer routing is disabled by the operator; events this channel's routing does not give to the default consumer are refused."
    };
    format!("<p class=\"notice\" role=\"status\">{text}</p>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_banner_names_loading_progress_and_a_partial_index() {
        assert_eq!(index(None, false), "");
        assert!(index(Some(300), false).contains("Loading channels… 300"));
        let partial = index(None, true);
        assert!(partial.contains("Partial index"));
        assert!(partial.contains("may be missing"));
    }

    #[test]
    fn source_banners_explain_absent_controls() {
        assert_eq!(sources(SourceAvailability::Available), "");
        assert!(sources(SourceAvailability::Disabled)
            .contains("Source changes are disabled by the operator."));
        assert!(sources(SourceAvailability::NotYetAvailable).contains("not available"));
    }

    #[test]
    fn routing_banners_appear_only_for_routed_channels_with_routing_off() {
        assert_eq!(routing(false, false, false), "");
        assert_eq!(routing(true, false, true), "");
        assert!(routing(true, false, false).contains("are refused"));
        assert!(routing(true, true, false).contains("All events continue to the default consumer"));
    }
}

//! The frame of a channel's detail page at `GET /console/channels/{id}`:
//! the channel's name and record, the six tabs, and the chosen tab's
//! content.
use cmaeg_core::ids::RecordId;

use super::{channels::chip, channels::status_label, escape};
use crate::console::{directory::ChannelView, redirect::HOME};

/// The route template of the detail page.
pub(crate) const PATH: &str = "/console/channels/{record_id}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Overview,
    Sources,
    Consumers,
    Routing,
    Activity,
    Producers,
}

const TABS: [Tab; 6] = [
    Tab::Overview,
    Tab::Sources,
    Tab::Consumers,
    Tab::Routing,
    Tab::Activity,
    Tab::Producers,
];

impl Tab {
    /// The `tab` query value; anything unknown shows the overview.
    pub(crate) fn parse(value: Option<&str>) -> Self {
        TABS.into_iter()
            .find(|tab| Some(tab.slug()) == value)
            .unwrap_or(Self::Overview)
    }

    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Sources => "sources",
            Self::Consumers => "consumers",
            Self::Routing => "routing",
            Self::Activity => "activity",
            Self::Producers => "producers",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Sources => "Sources",
            Self::Consumers => "Consumers",
            Self::Routing => "Routing",
            Self::Activity => "Activity",
            Self::Producers => "Producers",
        }
    }
}

/// The same-origin address of `record`'s detail page.
pub(crate) fn path(record: &RecordId) -> String {
    format!("{HOME}/channels/{record}")
}

/// The same-origin address of `record`'s `tab`.
pub(crate) fn href(record: &RecordId, tab: Tab) -> String {
    format!("{}?tab={}", path(record), tab.slug())
}

/// The page title: the channel's name, or its record when it has none.
pub(crate) fn title(view: &ChannelView) -> String {
    if view.summary.name.is_empty() {
        format!("Channel {}", view.record)
    } else {
        view.summary.name.clone()
    }
}

/// The header, the tab bar and `content`.
pub(crate) fn body(view: &ChannelView, tab: Tab, content: &str) -> String {
    let tabs: String = TABS
        .into_iter()
        .map(|item| {
            let current = if item == tab {
                " aria-current=\"page\""
            } else {
                ""
            };
            format!(
                "<a href=\"{}\"{current}>{}</a>",
                escape(&href(&view.record, item)),
                item.title()
            )
        })
        .collect();
    format!(
        "<p><a href=\"{HOME}\">All channels</a></p><h1>{}</h1><p class=\"lead\"><code>{}</code> {}</p><nav class=\"tabs\" aria-label=\"Channel\">{tabs}</nav>{content}",
        escape(&title(view)),
        escape(view.record.as_str()),
        chip(status_label(&view.summary), status_label(&view.summary)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_parse_by_slug_and_fall_back_to_the_overview() {
        for tab in TABS {
            assert_eq!(Tab::parse(Some(tab.slug())), tab);
        }
        assert_eq!(Tab::parse(Some("secrets")), Tab::Overview);
        assert_eq!(Tab::parse(None), Tab::Overview);
    }
}

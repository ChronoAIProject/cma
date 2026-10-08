//! The read-only Routing tab: which consumers an event goes to. Routing is
//! changed through the REST API.
use super::{banners, escape, facts};
use crate::console::directory::ChannelView;

pub(crate) fn render(view: &ChannelView, multi_consumer: bool) -> String {
    let summary = &view.summary;
    let mut html = banners::routing(
        summary.routed,
        summary.default_receives_all == Some(true),
        multi_consumer,
    );
    if summary.routed {
        html.push_str("<p class=\"empty\">This channel has a routing policy this gateway release cannot display.</p>");
        return html;
    }
    let default = summary
        .consumers
        .first()
        .map_or_else(|| "default".to_owned(), |consumer| consumer.label.clone());
    html.push_str("<p class=\"lead\">This channel has no routing policy: every admitted event goes to its default consumer, and its replies go back to the source the event came from.</p>");
    html.push_str(&facts(&[
        ("Model", "legacy".to_owned()),
        ("Default targets", escape(&default)),
        ("Rules", "none".to_owned()),
        ("Default consumer receives every event", "yes".to_owned()),
    ]));
    html
}

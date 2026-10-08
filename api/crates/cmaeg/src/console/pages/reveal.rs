//! The one page that shows a secret: the HMAC secret the gateway generated
//! for an attached custom source. It is the response to the attach POST
//! itself (the one exception to redirect-after-POST), carries
//! `Cache-Control: no-store` and the console's CSP like every console
//! response, and keeps the value inside a native `<details>` whose summary
//! shows only its last four characters. The value is never put in a URL, a
//! `Location`, a flash, the session, a cache or a log; refreshing re-posts a
//! stale version and gets 409, so the secret is never minted twice.
use cmaeg_core::ids::RecordId;

use super::{
    channel::{href, Tab},
    escape,
};
use crate::console::mask;

pub(crate) struct RevealView<'a> {
    pub record: &'a RecordId,
    pub source_id: &'a str,
    pub key_id: &'a str,
    pub secret: &'a str,
}

pub(crate) fn render(reveal: &RevealView<'_>) -> String {
    format!(
        "<h1>Source attached</h1><p class=\"lead\">The custom source <code>{}</code> is attached. The gateway generated the HMAC secret for key id <code>{}</code>; this page is the only time it is shown.</p><details class=\"reveal\"><summary>Show the secret ending {}</summary><p><code>{}</code></p><p>Copy it into the producer now. Reloading this page will not show it again.</p></details><p><a class=\"button\" href=\"{}\">Done</a></p>",
        escape(reveal.source_id),
        escape(reveal.key_id),
        escape(&mask::actor(reveal.secret)),
        escape(reveal.secret),
        escape(&href(reveal.record, Tab::Sources)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::test_support::record;

    #[test]
    fn the_secret_sits_only_inside_details_and_its_summary_shows_four_characters() {
        let html = render(&RevealView {
            record: &record(1),
            source_id: "custom",
            key_id: "k1",
            secret: "abcdefghijklmnopWXYZ",
        });
        let (before, inside) = html.split_once("<details class=\"reveal\">").unwrap();
        assert!(!before.contains("abcdefghijklmnopWXYZ"));
        let (summary, rest) = inside.split_once("</summary>").unwrap();
        assert!(summary.contains("…WXYZ"));
        assert!(!summary.contains("abcdefghijklmnop"));
        let (content, after) = rest.split_once("</details>").unwrap();
        assert!(content.contains("<code>abcdefghijklmnopWXYZ</code>"));
        assert!(!after.contains("abcdefghijklmnopWXYZ"));
    }
}

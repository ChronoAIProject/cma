//! The signed-in layout: the masked subject, the owner switcher, sign-out,
//! a one-time notice and the main area the channel pages fill.
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use super::{document, escape, post_button};
use crate::console::{
    directory::ChannelPage,
    redirect::{LOGOUT, OWNER},
    session::{Flash, OrgChoice, OwnerSelection},
};

pub(crate) const PERSONAL: &str = "personal";

pub(crate) struct LayoutView<'a> {
    /// Already masked.
    pub subject: &'a str,
    pub selected: &'a OwnerSelection,
    pub orgs: &'a [OrgChoice],
    pub owner_csrf: &'a str,
    pub logout_csrf: &'a str,
    pub flash: Option<&'a Flash>,
    pub channels: &'a ChannelPage,
}

pub(crate) fn render(view: &LayoutView<'_>) -> Response {
    let account = format!(
        "<div class=\"account\"><span>Signed in as {}</span>{}{}</div>",
        escape(view.subject),
        switcher(view),
        post_button(LOGOUT, view.logout_csrf, "Sign out", "secondary"),
    );
    let owner = match view.selected {
        OwnerSelection::Personal => "your personal account".to_owned(),
        OwnerSelection::Organization(id) => format!("organization {}", org_name(view.orgs, id)),
    };
    let notice = view.flash.map_or_else(String::new, |flash| {
        format!(
            "<p class=\"notice\" role=\"status\">{}</p>",
            escape(&flash_text(flash, view.orgs))
        )
    });
    let body = format!(
        "{notice}<h1>Channels</h1><p class=\"lead\">Channels of {}.</p>{}",
        escape(&owner),
        channels(view.channels),
    );
    (StatusCode::OK, Html(document("Console", &account, &body))).into_response()
}

/// "Personal" plus every organization NyxID listed; only admin ones can be
/// chosen.
fn switcher(view: &LayoutView<'_>) -> String {
    let mut options = format!(
        "<option value=\"{PERSONAL}\"{}>Personal</option>",
        selected_attr(matches!(view.selected, OwnerSelection::Personal))
    );
    let mut listed = false;
    for org in view.orgs {
        let selected = matches!(view.selected, OwnerSelection::Organization(id) if *id == org.id);
        listed |= selected;
        if org.admin {
            options.push_str(&format!(
                "<option value=\"{}\"{}>{}</option>",
                escape(&org.id),
                selected_attr(selected),
                escape(&org.name)
            ));
        } else {
            options.push_str(&format!(
                "<option value=\"{}\" disabled>{} (Admin role required)</option>",
                escape(&org.id),
                escape(&org.name)
            ));
        }
    }
    if let (OwnerSelection::Organization(id), false) = (view.selected, listed) {
        options.push_str(&format!(
            "<option value=\"{}\" selected>{}</option>",
            escape(id),
            escape(id)
        ));
    }
    format!(
        "<form class=\"inline\" method=\"post\" action=\"{OWNER}\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><label for=\"owner\">Owner</label><select id=\"owner\" name=\"owner\">{options}</select><button type=\"submit\" class=\"secondary\">Switch</button></form>",
        escape(view.owner_csrf)
    )
}

fn selected_attr(selected: bool) -> &'static str {
    if selected {
        " selected"
    } else {
        ""
    }
}

fn org_name(orgs: &[OrgChoice], id: &str) -> String {
    orgs.iter()
        .find(|org| org.id == id)
        .map_or_else(|| id.to_owned(), |org| org.name.clone())
}

fn flash_text(flash: &Flash, orgs: &[OrgChoice]) -> String {
    match flash {
        Flash::AdminEnded { organization } => format!(
            "Your admin access to {} has ended",
            org_name(orgs, organization)
        ),
        Flash::OwnerRefused => "That owner could not be selected.".to_owned(),
    }
}

/// The main area. The channel pages replace this list.
fn channels(page: &ChannelPage) -> String {
    let mut html = String::new();
    if let Some(seen) = page.loading {
        html.push_str(&format!("<p class=\"notice\">Loading channels… {seen}</p>"));
    }
    if page.partial {
        html.push_str("<p class=\"notice\">Only part of the channel index is in memory; some channels may be missing.</p>");
    }
    if page.rows.is_empty() {
        html.push_str("<p class=\"empty\">No channels to show yet.</p>");
        return html;
    }
    html.push_str("<ul>");
    for row in &page.rows {
        html.push_str(&format!(
            "<li>{} <span class=\"empty\">{}</span></li>",
            escape(&row.name),
            escape(&row.record_id)
        ));
    }
    html.push_str("</ul>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::directory::ChannelRow;

    fn org(id: &str, name: &str, admin: bool) -> OrgChoice {
        OrgChoice {
            id: id.into(),
            name: name.into(),
            admin,
        }
    }

    async fn body(view: &LayoutView<'_>) -> String {
        let bytes = axum::body::to_bytes(render(view).into_body(), 1 << 20)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn the_switcher_offers_admin_organizations_and_disables_the_rest() {
        let orgs = [org("org-a", "Org <A>", true), org("org-b", "Org B", false)];
        let selected = OwnerSelection::Organization("org-a".into());
        let flash = Flash::AdminEnded {
            organization: "org-b".into(),
        };
        let channels = ChannelPage {
            rows: vec![ChannelRow {
                record_id: "rec_1".into(),
                name: "<script>x</script>".into(),
            }],
            total: 1,
            loading: Some(3),
            partial: true,
        };
        let page = body(&LayoutView {
            subject: "…lice",
            selected: &selected,
            orgs: &orgs,
            owner_csrf: "owner-token",
            logout_csrf: "logout-token",
            flash: Some(&flash),
            channels: &channels,
        })
        .await;
        assert!(page.contains("<option value=\"personal\">Personal</option>"));
        assert!(page.contains("<option value=\"org-a\" selected>Org &lt;A&gt;</option>"));
        assert!(
            page.contains("<option value=\"org-b\" disabled>Org B (Admin role required)</option>")
        );
        assert!(page.contains("Your admin access to Org B has ended"));
        assert!(page.contains("Channels of organization Org &lt;A&gt;."));
        assert!(page.contains("&lt;script&gt;x&lt;/script&gt;"));
        assert!(!page.contains("<script>"));
        assert!(page.contains("Loading channels… 3"));
        assert!(page.contains(
            "action=\"/console/owner\"><input type=\"hidden\" name=\"csrf\" value=\"owner-token\">"
        ));
        assert!(page.contains("action=\"/console/logout\"><input type=\"hidden\" name=\"csrf\" value=\"logout-token\">"));
    }

    #[tokio::test]
    async fn an_unlisted_selection_stays_visible_and_an_empty_owner_says_so() {
        let selected = OwnerSelection::Organization("org-z".into());
        let page = body(&LayoutView {
            subject: "…lice",
            selected: &selected,
            orgs: &[],
            owner_csrf: "c",
            logout_csrf: "c",
            flash: Some(&Flash::OwnerRefused),
            channels: &ChannelPage::default(),
        })
        .await;
        assert!(page.contains("<option value=\"org-z\" selected>org-z</option>"));
        assert!(page.contains("That owner could not be selected."));
        assert!(page.contains("No channels to show yet."));
    }
}

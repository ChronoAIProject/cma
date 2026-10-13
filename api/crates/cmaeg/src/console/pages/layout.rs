//! The signed-in layout: the masked subject, the owner switcher, sign-out,
//! a one-time notice and the main area every channel page fills.
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use super::{document, escape, post_button};
use crate::console::{
    redirect::{LOGOUT, OWNER},
    session::{Flash, OrgChoice, OwnerSelection},
};

pub(crate) const PERSONAL: &str = "personal";

pub(crate) struct Frame<'a> {
    /// Already masked.
    pub subject: &'a str,
    pub selected: &'a OwnerSelection,
    pub orgs: &'a [OrgChoice],
    pub owner_csrf: &'a str,
    pub logout_csrf: &'a str,
    pub flash: Option<&'a Flash>,
}

/// A complete signed-in page. `head` and `body` are markup the caller built
/// from escaped values.
pub(crate) fn render(
    frame: &Frame<'_>,
    status: StatusCode,
    title: &str,
    head: &str,
    body: &str,
) -> Response {
    let account = format!(
        "<div class=\"account\"><span>Signed in as {}</span>{}{}</div>",
        escape(frame.subject),
        switcher(frame),
        post_button(LOGOUT, frame.logout_csrf, "Sign out", "secondary"),
    );
    let notice = frame.flash.map_or_else(String::new, |flash| {
        format!(
            "<p class=\"notice\" role=\"status\">{}</p>",
            escape(&flash_text(flash, frame.orgs))
        )
    });
    (
        status,
        Html(document(title, head, &account, &format!("{notice}{body}"))),
    )
        .into_response()
}

/// Whose channels a page shows: "your personal account" or the selected
/// organization by name. Unescaped.
pub(crate) fn owner_label(selected: &OwnerSelection, orgs: &[OrgChoice]) -> String {
    match selected {
        OwnerSelection::Personal => "your personal account".to_owned(),
        OwnerSelection::Organization(id) => format!("organization {}", org_name(orgs, id)),
    }
}

/// "Personal" plus every organization NyxID listed; only admin ones can be
/// chosen.
fn switcher(frame: &Frame<'_>) -> String {
    let mut options = format!(
        "<option value=\"{PERSONAL}\"{}>Personal</option>",
        selected_attr(matches!(frame.selected, OwnerSelection::Personal))
    );
    let mut listed = false;
    for org in frame.orgs {
        let selected = matches!(frame.selected, OwnerSelection::Organization(id) if *id == org.id);
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
    if let (OwnerSelection::Organization(id), false) = (frame.selected, listed) {
        options.push_str(&format!(
            "<option value=\"{}\" selected>{}</option>",
            escape(id),
            escape(id)
        ));
    }
    format!(
        "<form class=\"inline\" method=\"post\" action=\"{OWNER}\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><label for=\"owner\">Owner</label><select id=\"owner\" name=\"owner\">{options}</select><button type=\"submit\" class=\"secondary\">Switch</button></form>",
        escape(frame.owner_csrf)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn org(id: &str, name: &str, admin: bool) -> OrgChoice {
        OrgChoice {
            id: id.into(),
            name: name.into(),
            admin,
        }
    }

    async fn body(frame: &Frame<'_>, content: &str) -> String {
        let response = render(frame, StatusCode::OK, "Console", "", content);
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
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
        let page = body(
            &Frame {
                subject: "…lice",
                selected: &selected,
                orgs: &orgs,
                owner_csrf: "owner-token",
                logout_csrf: "logout-token",
                flash: Some(&flash),
            },
            "<h1>Main</h1>",
        )
        .await;
        assert!(page.contains("<option value=\"personal\">Personal</option>"));
        assert!(page.contains("<option value=\"org-a\" selected>Org &lt;A&gt;</option>"));
        assert!(
            page.contains("<option value=\"org-b\" disabled>Org B (Admin role required)</option>")
        );
        assert!(page.contains("Your admin access to Org B has ended"));
        assert!(page.contains("<h1>Main</h1>"));
        assert!(!page.contains("<script>"));
        assert!(page.contains(
            "action=\"/console/owner\"><input type=\"hidden\" name=\"csrf\" value=\"owner-token\">"
        ));
        assert!(page.contains("action=\"/console/logout\"><input type=\"hidden\" name=\"csrf\" value=\"logout-token\">"));
        assert_eq!(
            owner_label(&selected, &orgs),
            "organization Org <A>",
            "the label is escaped where it is rendered"
        );
    }

    #[tokio::test]
    async fn an_unlisted_selection_stays_visible_with_its_notice() {
        let selected = OwnerSelection::Organization("org-z".into());
        let page = body(
            &Frame {
                subject: "…lice",
                selected: &selected,
                orgs: &[],
                owner_csrf: "c",
                logout_csrf: "c",
                flash: Some(&Flash::OwnerRefused),
            },
            "",
        )
        .await;
        assert!(page.contains("<option value=\"org-z\" selected>org-z</option>"));
        assert!(page.contains("That owner could not be selected."));
        assert_eq!(
            owner_label(&OwnerSelection::Personal, &[]),
            "your personal account"
        );
    }
}

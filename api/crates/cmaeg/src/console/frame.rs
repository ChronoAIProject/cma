//! What every signed-in page shows around its content: the masked subject,
//! the owner switcher, sign-out and a one-time notice. It is loaded from the
//! session after the request's `Authorized` was derived, and carries the
//! session's CSRF key so a page can mint the tokens of its own forms.
use std::sync::Arc;

use axum::{http::StatusCode, response::Response};

use super::{
    csrf,
    pages::layout::{self, Frame},
    redirect,
    routes::mask,
    session::{Flash, OrgChoice, OwnerSelection, SessionCell},
    Console,
};

pub(crate) struct SignedIn {
    /// The subject's last four characters.
    subject: String,
    selected: OwnerSelection,
    orgs: Vec<OrgChoice>,
    csrf_key: [u8; 32],
    flash: Option<Flash>,
}

impl SignedIn {
    /// The frame of `cell`'s next page. It takes the session's one-time
    /// notice, so the notice is shown exactly once.
    pub(crate) async fn load(console: &Console, cell: &Arc<SessionCell>) -> Self {
        let orgs = console.org_choices(cell).await;
        let mut meta = cell.meta();
        Self {
            subject: mask(&meta.subject),
            selected: meta.selected.clone(),
            orgs,
            csrf_key: meta.csrf_key,
            flash: meta.flash.take(),
        }
    }

    /// The CSRF value of the form posting to the route template `action`
    /// for `record` (empty for forms that name no record).
    pub(crate) fn csrf(&self, action: &str, record: &str) -> String {
        csrf::token(&self.csrf_key, action, record)
    }

    /// Whose channels the page shows, for its lead sentence.
    pub(crate) fn owner_label(&self) -> String {
        layout::owner_label(&self.selected, &self.orgs)
    }

    /// The complete page: `head` and `body` are markup the caller built from
    /// escaped values.
    pub(crate) fn render(
        &self,
        status: StatusCode,
        title: &str,
        head: &str,
        body: &str,
    ) -> Response {
        layout::render(
            &Frame {
                subject: &self.subject,
                selected: &self.selected,
                orgs: &self.orgs,
                owner_csrf: &self.csrf(redirect::OWNER, ""),
                logout_csrf: &self.csrf(redirect::LOGOUT, ""),
                flash: self.flash.as_ref(),
            },
            status,
            title,
            head,
            body,
        )
    }
}

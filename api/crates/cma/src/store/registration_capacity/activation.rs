//! The immutable cohort marker also admits the new Launch reservation format.

use mongodb::{bson::Document, ClientSession, Collection};

pub(crate) enum Error {
    Driver(mongodb::error::Error),
    Unreadable(layer2_core::StoreError),
}

impl Error {
    pub(super) fn into_store_error(self) -> layer2_core::StoreError {
        match self {
            Self::Driver(error) => {
                super::unavailable(&format!("read capacity activation: {error}"))
            }
            Self::Unreadable(error) => error,
        }
    }
}

pub(crate) async fn read(
    admissions: &Collection<Document>,
    session: Option<&mut ClientSession>,
) -> Result<bool, Error> {
    let query = admissions
        .find_one(super::capacity_key())
        .max_time(super::QUERY_TIME);
    let marker = match session {
        Some(session) => query.session(session).await,
        None => query.await,
    }
    .map_err(Error::Driver)?;
    if let Some(marker) = marker {
        super::registration_count(&marker).map_err(Error::Unreadable)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

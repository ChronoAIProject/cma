//! Agent Manager entry points for Profile catalog storage.
pub(super) use cma_am_catalog::{
    decode_record, draft_mutation, is_retained_profile, profile_document, revision_document,
    save_draft, ProfileDocumentInput,
};

#[cfg(test)]
#[path = "profile_retained_reader_tests.rs"]
mod retained_reader_tests;

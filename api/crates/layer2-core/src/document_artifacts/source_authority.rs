use super::{valid_digest, DocumentError, DocumentSourceReservation};

/// Authority is bound to the incarnation admitted before storage I/O.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentSourceFence {
    generation: u64,
    reservation_id: String,
}

impl DocumentSourceFence {
    pub fn new(generation: u64, reservation_id: String) -> Result<Self, DocumentError> {
        if generation == 0 || generation > i64::MAX as u64 || !valid_digest(&reservation_id) {
            return Err(DocumentError::Corrupt);
        }
        Ok(Self {
            generation,
            reservation_id,
        })
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn reservation_id(&self) -> &str {
        &self.reservation_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentSourceWrite {
    pub source: DocumentSourceReservation,
    pub fence: Option<DocumentSourceFence>,
}

impl DocumentSourceWrite {
    pub fn generation(&self) -> u64 {
        self.fence
            .as_ref()
            .map_or(1, DocumentSourceFence::generation)
    }
}

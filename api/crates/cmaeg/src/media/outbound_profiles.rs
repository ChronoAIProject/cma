//! Bounded format eligibility shared by native outbound adapters.
//! These checks do not decode payloads; each adapter owns its MIME and byte policy.
pub(crate) mod bmff;
pub(crate) mod image;
pub(crate) mod ogg;
pub(crate) mod video;

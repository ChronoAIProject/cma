//! Only native WhatsApp reaction safety records need the platform's thirty-day horizon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum InteractionRetention {
    #[default]
    Ordinary,
    WhatsappReaction,
}
impl InteractionRetention {
    pub(super) fn maximum_age(self) -> i64 {
        match self {
            Self::Ordinary => 7 * 86400,
            Self::WhatsappReaction => cmaeg_core::seal::whatsapp_reaction::MAX_REACTION_AGE,
        }
    }
}

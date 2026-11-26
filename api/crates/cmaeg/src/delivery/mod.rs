//! Neutral delivery intents; the transport engine owns external sends.
mod activity_observation;
mod complete;
#[cfg(test)]
pub(crate) mod complete_tests;
pub mod custom;
mod default_reply;
pub mod destination;
mod discord;
mod discord_card_edit;
#[cfg(test)]
mod discord_card_edit_tests;
mod discord_card_media;
mod discord_card_replacement;
#[cfg(test)]
mod discord_card_replacement_fixture;
#[cfg(test)]
mod discord_card_replacement_tests;
mod discord_controls;
#[cfg(test)]
mod discord_controls_tests;
pub(crate) mod discord_edit;
#[cfg(test)]
mod discord_edit_tests;
#[cfg(test)]
mod discord_form_tests;
#[cfg(test)]
mod discord_media_card_edit_tests;
#[cfg(test)]
mod discord_media_controls_tests;
#[cfg(test)]
mod discord_media_set_fixture;
#[cfg(test)]
mod discord_media_set_tests;
#[cfg(test)]
mod discord_reaction_tests;
mod discord_receipts;
#[cfg(test)]
mod discord_tests;
mod discord_voice;
pub mod durable;
mod edit;
pub mod engine;
mod event;
pub mod gate;
pub mod intents;
mod interactive;
pub mod local_stream;
mod native_clients;
mod native_lark;
mod native_lark_card_edit;
mod native_lark_card_media;
mod native_lark_controls;
mod native_lark_thread_start;
mod pacing;
pub mod queue;
pub mod relay_client;
pub mod render;
pub(crate) mod retained_receipt;
pub mod router;
mod send;
pub(crate) mod side_messages;
mod split_text;
mod step;
pub mod sync;
mod telegram;
mod telegram_media;
mod telegram_media_card_edit;
#[cfg(test)]
mod telegram_media_card_edit_tests;
mod telegram_media_controls;
#[cfg(test)]
mod telegram_media_controls_fixture;
#[cfg(test)]
mod telegram_media_controls_tests;
#[cfg(test)]
mod telegram_media_tests;
mod telegram_structured;
#[cfg(test)]
mod telegram_structured_tests;
mod trace;
pub mod webhook_client;
mod whatsapp;
mod whatsapp_controls;
mod worker;

mod lark_effects;

mod native_media_ref;

#[cfg(test)]
mod split_text_tests;

#[cfg(test)]
mod discord_opus_voice_tests;
#[cfg(test)]
mod discord_voice_tests;

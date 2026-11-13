//! The schema mirrors the producer's static refusal vocabulary, never its
//! provider-controlled input. Inline enums keep the additive wire shape small.
use utoipa::openapi::schema::{Object, ObjectBuilder, Type};

pub(super) fn operation() -> Object {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["responses", "conversation"]))
        .build()
}

pub(super) fn code() -> Object {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some([
            "media_source_refused",
            "partition_agent_missing",
            "partition_agent_unavailable",
            "agent_command_admission_conflict",
            "agent_command_key_conflict",
            "binding_not_ready",
            "binding_resources_unavailable",
            "command_authority_required",
            "response_forbidden",
            "response_not_found",
            "model_mismatch",
            "unrecognized",
        ]))
        .build()
}

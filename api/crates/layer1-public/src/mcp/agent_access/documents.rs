use super::*;
use layer2_core::document_artifacts::{CreateDocument, DocumentError, UpdateDocument};

pub(super) const INSTRUCTIONS: &str = "When producing a specification, reference document, substantial explanation or other document to preserve for the user, call publish_document with its complete Markdown and return the confirmed artifact link. Use update_document for revisions to documents you authored; retain a stable request_key for identical transport retries. Documents are private in the owner's registry until the owner shares them. Never include secrets or publish external data without task authority. A failed save is not a published artifact.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
    request_key: String,
    title: String,
    markdown: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    request_key: String,
    document_id: String,
    expected_revision: u64,
    title: Option<String>,
    markdown: Option<String>,
}

pub(super) async fn invoke(
    state: &AccessState,
    capability: &str,
    id: Value,
    name: &str,
    arguments: Value,
) -> Response {
    let Some(documents) = &state.documents else {
        return rpc_error(id, -32602, "Unknown tool");
    };
    let outcome = if name == "publish_document" {
        let input: Publish = match serde_json::from_value(arguments) {
            Ok(v) => v,
            Err(_) => return rpc_error(id, -32602, "Invalid document arguments"),
        };
        documents
            .create_for_agent(
                state.auth.as_ref(),
                capability,
                &input.request_key,
                CreateDocument {
                    title: input.title,
                    markdown: input.markdown,
                    origin_agent_id: None,
                },
            )
            .await
    } else {
        let input: Update = match serde_json::from_value(arguments) {
            Ok(v) => v,
            Err(_) => return rpc_error(id, -32602, "Invalid document arguments"),
        };
        documents
            .update_for_agent(
                state.auth.as_ref(),
                capability,
                &input.document_id,
                &input.request_key,
                UpdateDocument {
                    expected_revision: input.expected_revision,
                    title: input.title,
                    markdown: input.markdown,
                },
            )
            .await
    };
    match outcome {
        Ok(document) => {
            let url = format!("{}/artifacts/{}", state.public_origin, document.id);
            result(
                id,
                json!({"content":[{"type":"text","text":format!("Saved document [{}]({url}). Document ID: {}; revision: {}. Include this link in your response.", document.title, document.id, document.revision)}],
                "structuredContent":{"document":document,"url":url},"isError":false}),
            )
        }
        Err(error) => {
            let instruction = match error {
                DocumentError::Unavailable => {
                    "Save could not be confirmed. Retry the exact same request_key and content; do not claim the document was published."
                }
                DocumentError::Conflict => {
                    "The document changed. Preserve the owner's newer edits; do not overwrite with an old revision."
                }
                DocumentError::NotFound => {
                    "This Agent has no authority to update that document or is no longer active. Do not retry with another owner's document."
                }
                _ => "The document was not saved. Check its size, request key, title and Markdown.",
            };
            result(
                id,
                json!({"content":[{"type":"text","text":instruction}],"isError":true}),
            )
        }
    }
}
pub(super) fn list() -> Vec<Value> {
    let key = json!({"type":"string","minLength":1,"maxLength":255,"description":"Stable key for this exact save. Reuse unchanged after a transport error; new edits need a new key."});
    let title = json!({"type":"string","minLength":1,"maxLength":240});
    let markdown = json!({"type":"string","minLength":1,"maxLength":1048576,"description":"Complete UTF-8 Markdown, at most 1 MiB. Supports tables, checklists, code fences, images and mermaid diagrams. Never send credentials."});
    vec![
        json!({"name":"publish_document","title":"Publish a document artifact","description":INSTRUCTIONS,
            "inputSchema":{"type":"object","additionalProperties":false,"required":["request_key","title","markdown"],"properties":{"request_key":key,"title":title,"markdown":markdown}},
            "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}}),
        json!({"name":"update_document","title":"Update your document artifact","description":"Save a new Markdown or title revision of a document created by this Agent. Existing revisions remain accessible. Uses expected_revision to preserve concurrent owner edits; cannot edit sharing, comment or another Agent's documents.",
            "inputSchema":{"type":"object","additionalProperties":false,"required":["request_key","document_id","expected_revision"],"properties":{"request_key":key,"document_id":{"type":"string","pattern":"^doc_[a-f0-9]{32}$"},"expected_revision":{"type":"integer","minimum":1},"title":title,"markdown":markdown}},
            "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}}),
    ]
}

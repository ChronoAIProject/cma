use super::*;
use crate::{
    net::exchange::{BodyError, HttpReply, TransportError},
    whatsapp::{tests::credentials, Transport, GRAPH_API_VERSION},
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, StatusCode, Url};
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

type Call = (Method, Url, HeaderMap, Value);
struct Graph {
    calls: Mutex<Vec<Call>>,
    replies: Mutex<VecDeque<Result<HttpReply, TransportError>>>,
}
impl Graph {
    fn new(replies: Vec<Result<HttpReply, TransportError>>) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::default(),
            replies: Mutex::new(replies.into()),
        })
    }
}
impl Transport for Graph {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        assert_eq!(timeout, Duration::from_secs(8));
        self.calls.lock().unwrap().push((
            method,
            url,
            headers,
            serde_json::from_slice(&body).unwrap(),
        ));
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected reaction resend");
        Box::pin(async move { reply })
    }
}
fn accepted() -> Value {
    json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"447700900123"}],"messages":[{"id":"wamid.reaction"}]})
}
fn answer(status: StatusCode, body: Value) -> Result<HttpReply, TransportError> {
    Ok(HttpReply::new(status, serde_json::to_vec(&body).unwrap()))
}
fn request(emoji: &str) -> MessageReactionRequest {
    MessageReactionRequest {
        reaction: if emoji.is_empty() {
            vec![]
        } else {
            vec![TelegramReaction::Emoji {
                emoji: emoji.into(),
            }]
        },
        is_big: false,
    }
}

#[tokio::test]
async fn exact_received_target_set_replace_and_clear_use_one_pinned_native_exchange() {
    for emoji in ["👍", "❤️", ""] {
        let graph = Graph::new(vec![answer(StatusCode::OK, accepted())]);
        let client = Client::production().unwrap().with_transport(graph.clone());
        assert_eq!(
            client
                .send_reaction(
                    &credentials(),
                    "34567",
                    "447700900123",
                    "wamid.original",
                    &request(emoji)
                )
                .await,
            Ok("wamid.reaction".into())
        );
        let calls = graph.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (method, url, headers, body) = &calls[0];
        assert_eq!(*method, Method::POST);
        assert_eq!(
            url.as_str(),
            format!("https://graph.facebook.com/{GRAPH_API_VERSION}/34567/messages")
        );
        assert!(headers["authorization"].is_sensitive());
        assert_eq!(
            headers["authorization"],
            format!("Bearer {}", credentials().access_token.as_str())
        );
        assert_eq!(headers["content-type"], "application/json");
        assert_eq!(
            *body,
            json!({"messaging_product":"whatsapp", "recipient_type":"individual", "to":"447700900123", "type":"reaction", "reaction":{"message_id":"wamid.original", "emoji":emoji}})
        );
    }
}

#[tokio::test]
async fn invalid_reaction_or_destination_never_contacts_meta() {
    for defect in 0..9 {
        let graph = Graph::new(vec![]);
        let client = Client::production().unwrap().with_transport(graph.clone());
        let mut credentials = credentials();
        let mut reaction = request("👍");
        let (mut phone, mut recipient, mut target) = ("34567", "447700900123", "wamid.original");
        match defect {
            0 => phone = "../messages",
            1 => recipient = "group:anywhere",
            2 => target = "not-a-message",
            3 => reaction.is_big = true,
            4 => reaction.reaction.push(TelegramReaction::Emoji {
                emoji: "❤️".into()
            }),
            5 => {
                reaction.reaction = vec![TelegramReaction::CustomEmoji {
                    custom_emoji_id: "12345".into(),
                }]
            }
            6 => reaction.reaction = vec![TelegramReaction::Emoji { emoji: "\n".into() }],
            7 => {
                reaction.reaction = vec![TelegramReaction::Emoji {
                    emoji: "👍".repeat(17),
                }]
            }
            8 => credentials.access_token = cmaeg_core::channel::Secret::new(""),
            _ => unreachable!(),
        }
        assert_eq!(
            client
                .send_reaction(&credentials, phone, recipient, target, &reaction)
                .await,
            Err(Error::Invalid),
            "defect {defect}"
        );
        assert!(graph.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn uncertain_reaction_acceptance_is_never_retried_or_mistaken_for_confirmation() {
    let mut wrong_recipient = accepted();
    wrong_recipient["contacts"][0]["wa_id"] = json!("447700900999");
    let mut multiple = accepted();
    multiple["messages"] = json!([{"id":"wamid.first"},{"id":"wamid.second"}]);
    let mut invalid_id = accepted();
    invalid_id["messages"][0]["id"] = json!("https://untrusted.example/message");
    for reply in [
        Err(TransportError::Unavailable),
        answer(StatusCode::OK, json!({})),
        answer(StatusCode::OK, wrong_recipient),
        answer(StatusCode::OK, multiple),
        answer(StatusCode::OK, invalid_id),
        answer(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":{"code":2}}),
        ),
        answer(StatusCode::TEMPORARY_REDIRECT, accepted()),
        Ok(HttpReply {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Err(BodyError::Lost),
        }),
    ] {
        let graph = Graph::new(vec![reply]);
        let client = Client::production().unwrap().with_transport(graph.clone());
        assert_eq!(
            client
                .send_reaction(
                    &credentials(),
                    "34567",
                    "447700900123",
                    "wamid.original",
                    &request("👍")
                )
                .await,
            Err(Error::Unconfirmed)
        );
        assert_eq!(graph.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn explicit_meta_rejection_and_rate_limit_remain_distinct_without_retry() {
    for (status, code, expected) in [
        (StatusCode::BAD_REQUEST, 131009, Error::Rejected),
        (StatusCode::UNAUTHORIZED, 190, Error::Rejected),
        (StatusCode::TOO_MANY_REQUESTS, 130429, Error::RateLimited),
        (StatusCode::BAD_REQUEST, 80007, Error::RateLimited),
    ] {
        let graph = Graph::new(vec![answer(status, json!({"error":{"code":code}}))]);
        let client = Client::production().unwrap().with_transport(graph.clone());
        assert_eq!(
            client
                .send_reaction(
                    &credentials(),
                    "34567",
                    "447700900123",
                    "wamid.original",
                    &request("👍")
                )
                .await,
            Err(expected)
        );
        assert_eq!(graph.calls.lock().unwrap().len(), 1);
    }
}

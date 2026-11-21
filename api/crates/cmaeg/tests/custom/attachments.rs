//! Attachment items declared in custom CloudEvent data, under the owner's setting.
use super::support::App;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::ingress::custom::attachments::{extract, MAX_INLINE_ATTACHMENT_BYTES};
use cmaeg_core::{channel::CustomAttachments, cloudevent::CloudEvent};
use serde_json::{json, Value};

const ORIGIN: &str = "https://files.example";
const TEMPLATE: &str = "{{type}} for {{subject}}: {{data}}";

/// Bytes shaped like a PNG; ingress never inspects them.
fn png() -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend((0..=255_u8).cycle().take(300));
    bytes
}

fn data_uri(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", STANDARD.encode(bytes))
}

/// A custom channel opting in at `pointer`, optionally rendering a template.
fn configured(pointer: &'static str, template: Option<&'static str>) -> impl FnOnce(&mut Value) {
    move |value: &mut Value| {
        value["sources"][0]["attachments"] = json!({"pointer":pointer,"origins":[ORIGIN]});
        if let Some(template) = template {
            value["sources"][0]["prompt_template"] = template.into();
        }
    }
}

fn event(id: &str, data: Value) -> Value {
    json!({"specversion":"1.0","id":id,"type":"order.created",
        "source":"https://orders.example","subject":"customer-1","data":data})
}

/// The dump a channel without a template renders, exactly as before attachments.
fn dump(data: &str) -> String {
    format!("Event order.created from https://orders.example:\n{data}")
}

async fn refused(app: &App, data: Value, detail: &str) {
    let response = app
        .request(&app.url, event("refused", data).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let problem: Value = response.json().await.unwrap();
    assert_eq!(problem["code"], "invalid_request");
    assert_eq!(problem["detail"], detail);
    assert!(app.provider.requests().is_empty());
    assert!(app.waiters.is_empty());
}

/// Send one event, wait for the turn, and return the provider's turn request.
async fn admitted(app: &mut App, data: Value) -> Value {
    let response = app
        .request(&app.url, event("admitted", data).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    app.drain().await;
    let post = app
        .provider
        .requests()
        .into_iter()
        .find(|request| request.method == "POST")
        .expect("the turn reached the provider");
    serde_json::from_str(&post.body).unwrap()
}

fn input(turn: &Value) -> &str {
    turn["input"][0]["content"][0]["text"].as_str().unwrap()
}

fn attachments(turn: &Value) -> &Value {
    &turn["event_context"]["activity"]["kind"]["attachments"]
}

#[tokio::test]
async fn inline_png_is_accepted() {
    let bytes = png();
    let uri = data_uri("image/png", &bytes);
    let encoded = STANDARD.encode(&bytes);
    let mut app = App::media(configured("/attachments", None), true).await;
    let turn = admitted(
        &mut app,
        json!({"order":7,"attachments":[{"url":uri,"filename":"receipt.png"}]}),
    )
    .await;
    let text = input(&turn);
    assert!(text.contains("[Attachments: 1"), "{text}");
    assert!(!text.contains(&encoded[..24]), "{text}");
    assert!(!text.contains("base64"), "{text}");
    let item = &attachments(&turn)[0];
    assert_eq!(
        item["source"],
        json!({"type":"inline","pointer":"/attachments/0/url"})
    );
    assert_eq!(item["size_bytes"], bytes.len());
    assert_eq!(item["mime_type"], "image/png");
    assert_eq!(item["kind"], "image");
    assert_eq!(item["reference"], "custom:0");
    assert_eq!(item["filename"], "receipt.png");
}

#[tokio::test]
async fn allowlisted_https_origin_is_accepted() {
    let mut app = App::media(configured("/attachments", None), true).await;
    let url = format!("{ORIGIN}/reports/q3.pdf?version=2");
    let turn = admitted(
        &mut app,
        json!({"attachments":[{"url":url,"mime_type":"application/pdf","size_bytes":4096}]}),
    )
    .await;
    let item = &attachments(&turn)[0];
    assert_eq!(item["source"], json!({"type":"url","url":url}));
    assert_eq!(item["kind"], "file");
    assert_eq!(item["size_bytes"], 4096);
    let text = input(&turn);
    assert!(text.contains("[Attachments: 1"), "{text}");
    assert!(!text.contains("files.example"), "{text}");
}

#[tokio::test]
async fn disallowed_origin_is_400() {
    let app = App::media(configured("/attachments", None), true).await;
    for url in [
        "https://other.example/a.pdf",
        "https://files.example.evil.example/a.pdf",
        "http://files.example/a.pdf",
    ] {
        refused(
            &app,
            json!({"attachments":[{"url":url}]}),
            "attachment_invalid /attachments/0/url",
        )
        .await;
    }
}

#[tokio::test]
async fn inline_over_192_kib_is_400() {
    // Base64 of more than 192 KiB alone exceeds the 256 KiB event body, so
    // the decoded bound is enforced by the extractor and the body cap answers
    // first on the wire.
    let setting = CustomAttachments {
        pointer: "/attachments".into(),
        origins: vec![],
    };
    let over = vec![0_u8; MAX_INLINE_ATTACHMENT_BYTES + 1];
    assert_eq!(over.len(), 196_609);
    let ce = |bytes: &[u8]| -> CloudEvent {
        serde_json::from_value(event(
            "inline",
            json!({"attachments":[{"url":data_uri("application/octet-stream", bytes)}]}),
        ))
        .unwrap()
    };
    assert_eq!(
        extract(&ce(&over), &setting).unwrap_err().pointer,
        "/attachments/0/url"
    );
    let largest = extract(&ce(&over[1..]), &setting).unwrap();
    assert_eq!(largest[0].size_bytes, Some(196_608));

    let app = App::media(configured("/attachments", None), true).await;
    let body = event(
        "inline",
        json!({"attachments":[{"url":data_uri("application/octet-stream", &over)}]}),
    );
    let response = app
        .request(&app.url, body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    assert!(app.provider.requests().is_empty());
}

#[tokio::test]
async fn mismatched_inline_size_is_400() {
    let app = App::media(configured("/attachments", None), true).await;
    refused(
        &app,
        json!({"attachments":[{"url":data_uri("image/png", &png()),"size_bytes":1}]}),
        "attachment_invalid /attachments/0/size_bytes",
    )
    .await;
}

#[tokio::test]
async fn unknown_item_key_is_400_with_pointer() {
    let app = App::media(configured("/attachments", None), true).await;
    refused(
        &app,
        json!({"attachments":[{"url":data_uri("image/png", &png()),"caption":"hi"}]}),
        "attachment_invalid /attachments/0/caption",
    )
    .await;
    // A sender-chosen key is bounded in the answer.
    let key = "k".repeat(300);
    let response = app
        .request(
            &app.url,
            event(
                "long",
                json!({"attachments":[{"url":"data:a/b;base64,", key:1}]}),
            )
            .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let problem: Value = response.json().await.unwrap();
    let detail = problem["detail"].as_str().unwrap();
    assert_eq!(detail.len(), 256);
    assert!(detail.starts_with("attachment_invalid /attachments/0/kkk"));
}

#[tokio::test]
async fn long_filename_is_400() {
    let app = App::media(configured("/attachments", None), true).await;
    refused(
        &app,
        json!({"attachments":[{"url":data_uri("image/png", &png()),"filename":"f".repeat(300)}]}),
        "attachment_invalid /attachments/0/filename",
    )
    .await;
}

#[tokio::test]
async fn zero_width_is_400() {
    let app = App::media(configured("/attachments", None), true).await;
    refused(
        &app,
        json!({"attachments":[{"url":data_uri("image/png", &png()),"width":0,"height":10}]}),
        "attachment_invalid /attachments/0/width",
    )
    .await;
}

#[tokio::test]
async fn seventeen_items_is_400() {
    let app = App::media(configured("/attachments", None), true).await;
    let item = json!({"url":"data:text/plain;base64,aGk="});
    let sixteen: Vec<Value> = std::iter::repeat_n(item.clone(), 16).collect();
    let mut seventeen = sixteen.clone();
    seventeen.push(item);
    refused(
        &app,
        json!({ "attachments": seventeen }),
        "attachment_invalid /attachments",
    )
    .await;
    let mut app = App::media(configured("/attachments", None), true).await;
    let turn = admitted(&mut app, json!({ "attachments": sixteen })).await;
    assert_eq!(attachments(&turn).as_array().unwrap().len(), 16);
    assert!(input(&turn).contains("[16 attachments, listed below]"));
}

#[tokio::test]
async fn non_array_target_is_400() {
    let app = App::media(configured("/attachments", None), true).await;
    for target in [
        json!({"url":"data:text/plain;base64,aGk="}),
        json!("x"),
        json!(1),
    ] {
        refused(
            &app,
            json!({ "attachments": target }),
            "attachment_invalid /attachments",
        )
        .await;
    }
}

#[tokio::test]
async fn flag_off_ignores_config() {
    let uri = data_uri("image/png", &png());
    let data = json!({"order":7,"attachments":[{"url":uri.clone(),"bogus":true}]});
    let canonical =
        format!("{{\"attachments\":[{{\"bogus\":true,\"url\":\"{uri}\"}}],\"order\":7}}");
    let mut app = App::media(configured("/attachments", None), false).await;
    let turn = admitted(&mut app, data.clone()).await;
    assert_eq!(input(&turn), dump(&canonical));
    assert!(turn["event_context"]["activity"]["kind"]
        .get("attachments")
        .is_none());
    let mut app = App::media(configured("/attachments", Some(TEMPLATE)), false).await;
    let turn = admitted(&mut app, data).await;
    assert_eq!(
        input(&turn),
        format!("order.created for customer-1: {canonical}")
    );
}

#[tokio::test]
async fn no_base64_in_prompt() {
    let bytes = png();
    let encoded = STANDARD.encode(&bytes);
    let data = json!({"order":7,"attachments":[{"url":data_uri("image/png", &bytes)}]});
    let elided = "{\"attachments\":\"[1 attachments, listed below]\",\"order\":7}";
    for template in [None, Some(TEMPLATE)] {
        let mut app = App::media(configured("/attachments", template), true).await;
        let turn = admitted(&mut app, data.clone()).await;
        let text = input(&turn);
        let expected = match template {
            None => dump(elided),
            Some(_) => format!("order.created for customer-1: {elided}"),
        };
        assert!(
            text.starts_with(&format!("{expected}\n\n[Attachments: 1")),
            "{text}"
        );
        assert!(text.contains("[1 attachments, listed below]"));
        for window in encoded.as_bytes().windows(16).step_by(16) {
            assert!(
                !text.contains(std::str::from_utf8(window).unwrap()),
                "{text}"
            );
        }
    }
}

#[tokio::test]
async fn stored_config_without_attachments_renders_unchanged() {
    for data in [
        json!({"z":1,"a":2}),
        json!({"z":1,"a":2,"attachments":null}),
    ] {
        let mut app = App::media(configured("/attachments", None), true).await;
        let turn = admitted(&mut app, data.clone()).await;
        let canonical = if data.get("attachments").is_some() {
            "{\"a\":2,\"attachments\":null,\"z\":1}"
        } else {
            "{\"a\":2,\"z\":1}"
        };
        assert_eq!(input(&turn), dump(canonical));
        assert!(turn["event_context"]["activity"]["kind"]
            .get("attachments")
            .is_none());
    }
}

#[tokio::test]
async fn custom_pointer_is_honoured() {
    let mut app = App::media(configured("/media/items", None), true).await;
    let turn = admitted(
        &mut app,
        json!({"note":"x","attachments":"not the configured pointer",
            "media":{"items":[{"url":"data:text/plain;base64,aGVsbG8="}]}}),
    )
    .await;
    let item = &attachments(&turn)[0];
    assert_eq!(item["source"]["pointer"], "/media/items/0/url");
    assert_eq!(item["kind"], "file");
    assert_eq!(item["size_bytes"], 5);
    assert!(input(&turn).starts_with(&dump(
        "{\"attachments\":\"not the configured pointer\",\"media\":{\"items\":\"[1 attachments, listed below]\"},\"note\":\"x\"}"
    )));
}

/// Render one event activity through the provider input renderer.
fn rendered(definition: &serde_json::Value, data: Value, items: Value) -> String {
    use cmaeg::turns::{
        render_input,
        render_media::{Audience, RenderMedia},
    };
    let mut activity: serde_json::Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/activity.json"
    ))
    .unwrap();
    activity["source"] = json!({"type":"custom"});
    activity["kind"] = json!({"type":"event","ce":event("rendered", data),"attachments":items});
    let input = render_input::render(
        &serde_json::from_value(activity).unwrap(),
        &serde_json::from_value(definition.clone()).unwrap(),
        &RenderMedia::empty(None),
        Audience {
            tools: false,
            downloads: false,
        },
    );
    input[0]["content"][0]["text"].as_str().unwrap().to_owned()
}

#[test]
fn elision_follows_the_pointer_an_inline_item_was_admitted_under() {
    let mut definition: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    let data = json!({"media":["data:text/plain;base64,aGk="],"other":["kept"]});
    let inline = json!([{"reference":"custom:0","kind":"file",
        "source":{"type":"inline","pointer":"/media/0/url"}}]);
    let elided = dump("{\"media\":\"[1 attachments, listed below]\",\"other\":[\"kept\"]}");
    // The setting was removed, then moved, after the event was admitted.
    assert!(rendered(&definition, data.clone(), inline.clone()).starts_with(&elided));
    definition["sources"][0]["attachments"] = json!({"pointer":"/other"});
    assert!(rendered(&definition, data.clone(), inline).starts_with(&elided));
    // URL items record no pointer, so the configured one applies.
    let url = json!([{"reference":"custom:0","kind":"file",
        "source":{"type":"url","url":"https://files.example/a"}}]);
    assert!(rendered(&definition, data.clone(), url.clone()).starts_with(&dump(
        "{\"media\":[\"data:text/plain;base64,aGk=\"],\"other\":\"[1 attachments, listed below]\"}"
    )));
    definition["sources"][0]
        .as_object_mut()
        .unwrap()
        .remove("attachments");
    assert!(rendered(&definition, data, url).starts_with(&dump(
        "{\"media\":[\"data:text/plain;base64,aGk=\"],\"other\":[\"kept\"]}"
    )));
}

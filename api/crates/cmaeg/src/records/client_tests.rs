use super::*;
#[tokio::test]
async fn record_urls_are_exact_and_never_send_service_tokens_to_other_origins() {
    let client = RecordClient::new("http://127.0.0.1:1", "service-fixture").unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let valid = client.record_url(&id).unwrap();
    assert!(!format!("{client:?}").contains("service-fixture"));
    for url in [
        format!("https://other.example{RECORD_PATH}/{id}"),
        format!("{valid}?query=1"),
        format!("{valid}#fragment"),
        format!("{valid}/extra"),
        valid.to_string().replace(&id, ".."),
        valid.to_string().replace("127.0.0.1", "localhost"),
    ] {
        assert!(matches!(
            client.get(&url).await,
            Err(RecordError::Rejected(_))
        ));
    }
    assert!(client.record_url("../other").is_err());
    assert!(client.record_url("not-a-uuid").is_err());
}

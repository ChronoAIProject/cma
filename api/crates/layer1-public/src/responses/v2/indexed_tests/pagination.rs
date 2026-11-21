use super::*;

#[tokio::test]
async fn oversized_result_is_explicit_and_every_output_page_remains_accessible() {
    let mut kinds = beginning();
    kinds.extend((0..12).map(|_| message(&"a".repeat(400_000))));
    kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    let f = make(kinds).await;
    let reading = f.reading.clone();
    assert!(f
        .observe(move |source| Box::pin(async move {
            projection::indexed::complete(
                source,
                &crate::agent_page::AgentPageOrigin::test(),
                reading,
            )
            .await
        }))
        .await
        .unwrap()
        .is_none());
    let count = f
        .observe(move |source| {
            Box::pin(async move {
                let mut from = 0;
                let mut count = 0;
                loop {
                    let page = projection::indexed::output_page(source, from).await?;
                    assert!(serde_json::to_vec(&page.items).unwrap().len() <= 4 * 1024 * 1024);
                    count += page.items.len();
                    if !page.more {
                        break;
                    }
                    assert!(page.next > from);
                    from = page.next;
                }
                Ok(count)
            })
        })
        .await
        .unwrap();
    assert_eq!(count, 12);
    let reading = f.reading.clone();
    let terminal = f
        .observe(move |source| {
            Box::pin(async move {
                stream_page::read(
                    source,
                    &crate::agent_page::AgentPageOrigin::test(),
                    reading,
                    2 + 12 * 6,
                )
                .await
            })
        })
        .await
        .unwrap();
    assert!(terminal.pagination_required && terminal.events.is_empty() && !terminal.terminal);

    use crate::stream_telemetry::{BodyEnd, CloseReason, Contract};
    let response = crate::responses::stream::open_authorized(
        f.state,
        crate::AuthenticatedResourceCaller {
            context: context("owner-a"),
            credential: None,
        },
        f.agent,
        3,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let evidence = response.extensions().get::<Contract>().unwrap().clone();
    let mut body = response.into_body().into_data_stream();
    let mut pagination = false;
    while let Some(frame) = body.next().await {
        let frame = frame.unwrap();
        pagination |= events(&frame)
            .iter()
            .any(|event| event["code"] == "response_pagination_required");
    }
    assert!(pagination);
    assert_eq!(
        evidence.close_reason(BodyEnd::Eof),
        CloseReason::PaginationRequired
    );
}

#[tokio::test]
async fn missing_proof_never_becomes_empty_output_or_legacy_fallback() {
    let mut kinds = beginning();
    kinds.extend([
        message("answer"),
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ]);
    let f = make(kinds).await;
    f.source.counts.fail.store(true, Ordering::SeqCst);
    let reading = f.reading.clone();
    let result = f
        .observe(move |source| {
            Box::pin(async move {
                projection::indexed::complete(
                    source,
                    &crate::agent_page::AgentPageOrigin::test(),
                    reading,
                )
                .await
            })
        })
        .await;
    assert!(matches!(result, Err(AgentReadError::Corrupt(_))));
    assert_eq!(f.source.counts.points.load(Ordering::SeqCst), 0);
}

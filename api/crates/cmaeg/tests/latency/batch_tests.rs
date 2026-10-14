use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_batch_prepares_before_invalidation_and_sends_signed_http() {
    let mut app = App::new().await;
    measure(app.callback("prime-batch", 0).await, 202).await;
    app.quiet().await;
    let before = app.cma.gets.load(Ordering::SeqCst);
    assert_eq!(app.state.resolver.cache_len(), 1);

    let mut prepared = 0;
    let requests = app.cold_callbacks((0..8).map(|lane| {
        assert_eq!(
            app.state.resolver.cache_len(),
            1,
            "the channel was invalidated before all requests were prepared"
        );
        assert_eq!(app.cma.gets.load(Ordering::SeqCst), before);
        prepared += 1;
        (format!("prepared-{lane}"), lane)
    }));
    assert_eq!(prepared, 8);
    assert_eq!(requests.len(), 8);
    assert_eq!(app.state.resolver.cache_len(), 0);
    assert_eq!(app.cma.gets.load(Ordering::SeqCst), before);

    let pending = requests
        .into_iter()
        .map(|request| async move { measure(request.send().await.unwrap(), 202).await });
    let samples = futures_util::future::join_all(pending).await;
    app.quiet().await;
    assert_eq!(app.cma.gets.load(Ordering::SeqCst) - before, 1);
    assert!(samples.iter().all(|sample| sample["restore"] > 0.5));
    assert_eq!(app.identity.jwks_calls.load(Ordering::SeqCst), 1);
    let admitted = app
        .provider
        .requests()
        .into_iter()
        .filter(|request| request.method == "POST")
        .count();
    assert_eq!(admitted, 9);

    // Request preparation must retain the real body/signature check over HTTP.
    let request = app.callback_request("tampered-batch", 0).build().unwrap();
    let mut body = request.body().unwrap().as_bytes().unwrap().to_vec();
    body.push(b' ');
    let mut tampered = request.try_clone().unwrap();
    *tampered.body_mut() = Some(body.into());
    let response = app.client.execute(tampered).await.unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(
        response.json::<Value>().await.unwrap()["detail"],
        "body_digest_mismatch"
    );
    measure(app.client.execute(request).await.unwrap(), 202).await;
    app.quiet().await;
    assert_eq!(
        app.provider
            .requests()
            .into_iter()
            .filter(|request| request.method == "POST")
            .count(),
        admitted + 1
    );
    app.drain().await;
}

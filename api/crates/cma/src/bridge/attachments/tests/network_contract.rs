use super::*;
use tokio::io::AsyncReadExt;

#[test]
fn grants_require_independent_exact_https_host_policy_and_bounded_lifetime() {
    let hosts = ConnectorHosts::new(&["media.example"]).unwrap();
    for url in [
        "opaque-provider-handle",
        "http://media.example/resource",
        "file:///resource",
        "https://media.example.evil.test/resource",
        "https://child.media.example/resource",
        "https://media.example./resource",
        "https://user@media.example/resource",
        "https://user:pass@media.example/resource",
        "https://media.example:8443/resource",
        "https://media.example/resource#fragment",
        "https://127.0.0.1/resource",
        "https://[::1]/resource",
        "https://2130706433/resource",
        "https://0x7f000001/resource",
        "https://media.example/\nresource",
        "https:\\media.example\\resource",
    ] {
        assert!(
            FetchGrant::issue(
                request(),
                &hosts,
                url,
                Mime::Png,
                128,
                Instant::now() + Duration::from_secs(1)
            )
            .is_err(),
            "{url}"
        );
    }
    for host in [
        "*.example",
        "media.example.",
        "127.0.0.1",
        "2130706433",
        "-media.example",
        "media..example",
        "MEDIA.example",
        "media.example/path",
    ] {
        assert!(ConnectorHosts::new(&[host]).is_err(), "{host}");
    }
    assert!(ConnectorHosts::new(&[]).is_err());
    assert!(ConnectorHosts::new(&["media.example"; 17]).is_err());
    for bytes in [0, MAX_BYTES + 1] {
        assert!(matches!(
            FetchGrant::issue(
                request(),
                &hosts,
                "https://media.example/resource",
                Mime::Png,
                bytes,
                Instant::now() + Duration::from_secs(1)
            ),
            Err(Unavailable::Bounds)
        ));
    }
    for deadline in [
        Instant::now(),
        Instant::now() + MAX_DURATION + Duration::from_secs(1),
    ] {
        assert!(matches!(
            FetchGrant::issue(
                request(),
                &hosts,
                "https://media.example/resource",
                Mime::Png,
                128,
                deadline
            ),
            Err(Unavailable::Deadline)
        ));
    }
}

#[test]
fn private_special_mapped_and_transition_addresses_are_never_public_targets() {
    for ip in [
        "0.0.0.0",
        "0.1.2.3",
        "10.0.0.1",
        "100.64.0.1",
        "100.127.255.254",
        "127.0.0.1",
        "127.255.255.254",
        "169.254.169.254",
        "172.16.0.1",
        "172.31.255.254",
        "192.0.0.1",
        "192.0.2.1",
        "192.88.99.1",
        "192.168.0.1",
        "198.18.0.1",
        "198.19.255.254",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "239.1.2.3",
        "240.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "::127.0.0.1",
        "::ffff:127.0.0.1",
        "::ffff:8.8.8.8",
        "64:ff9b::a00:1",
        "64:ff9b:1::1",
        "100::1",
        "2001::1",
        "2001:2::1",
        "2001:db8::1",
        "2002:7f00:1::",
        "3fff::1",
        "fc00::1",
        "fd00::1",
        "fe80::1",
        "ff02::1",
    ] {
        assert!(!public_address(ip.parse().unwrap()), "{ip}");
    }
    for ip in [
        "8.8.8.8",
        "1.1.1.1",
        "2606:4700:4700::1111",
        "2001:4860:4860::8888",
    ] {
        assert!(public_address(ip.parse().unwrap()), "{ip}");
    }
}

#[tokio::test]
async fn empty_oversized_or_mixed_dns_never_connects() {
    for addresses in [
        vec![],
        vec!["8.8.8.8"; 17],
        vec!["8.8.8.8", "127.0.0.1"],
        vec!["8.8.8.8", "::ffff:127.0.0.1"],
    ] {
        let mut f = Fixture::new();
        f.dns.addresses = addresses.iter().map(|ip| ip.parse().unwrap()).collect();
        assert_eq!(f.run().await.err(), Some(Unavailable::Address));
        assert!(!f.events().contains(&"connect"));
    }
}

#[tokio::test]
async fn rebinding_or_wrong_port_cannot_supply_bytes_from_unvetted_peer() {
    for peer in [
        "127.0.0.1:443",
        "1.1.1.1:443",
        "8.8.8.8:8443",
        "[::ffff:8.8.8.8]:443",
    ] {
        let mut f = Fixture::new();
        f.http.peer = peer.parse().unwrap();
        assert_eq!(f.run().await.err(), Some(Unavailable::Address));
        assert!(!f.events().contains(&"body"));
        assert_eq!(f.events().iter().filter(|e| **e == "resolve").count(), 1);
    }
}

#[tokio::test]
async fn all_vetted_addresses_are_pinned_and_only_one_resolution_occurs() {
    let mut f = Fixture::new();
    f.dns
        .addresses
        .push("2606:4700:4700::1111".parse().unwrap());
    f.http.peer = "[2606:4700:4700::1111]:443".parse().unwrap();
    assert!(f.run().await.is_ok());
    assert_eq!(
        f.http.targets.lock().unwrap()[0],
        ["8.8.8.8:443".parse::<SocketAddr>().unwrap(), f.http.peer]
    );
    assert_eq!(f.events().iter().filter(|e| **e == "resolve").count(), 1);
}

#[tokio::test]
async fn https_transport_connects_to_pin_with_original_tls_name_and_rejects_bad_tls() {
    // Construct a private target directly for this transport-only fixture. The
    // production PinnedTarget constructor rejects loopback and alternate ports.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let target = PinnedTarget {
        url: reqwest::Url::parse("https://media.example/resource").unwrap(),
        host: "media.example".into(),
        addresses: vec![address],
        deadline: Instant::now() + Duration::from_secs(3),
    };
    let observed = async {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut header = [0_u8; 5];
        socket.read_exact(&mut header).await.unwrap();
        assert_eq!(header[0], 22, "expected TLS handshake");
        let size = u16::from_be_bytes([header[3], header[4]]) as usize;
        assert!(size > 0 && size <= 8192);
        let mut hello = vec![0_u8; size];
        socket.read_exact(&mut hello).await.unwrap();
        assert!(hello
            .windows(b"media.example".len())
            .any(|s| s == b"media.example"));
        // Close before cert authentication; this cannot produce download bytes.
    };
    let (result, ()) = timeout_at(target.deadline, async {
        tokio::join!(network::HttpsTransport.get(&target), observed)
    })
    .await
    .expect("pinned listener was not contacted");
    assert!(matches!(result, Err(Unavailable::Transport)));
}

#[tokio::test]
async fn https_transport_ignores_inherited_proxy_configuration() {
    // Use a child process so proxy environment never races other test threads.
    // An inherited proxy at this closed endpoint would prevent the TLS fixture
    // from reaching its private pinned listener if no_proxy were removed.
    let name = "bridge::attachments::tests::network_contract::https_transport_connects_to_pin_with_original_tls_name_and_rejects_bad_tls";
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .env("https_proxy", "http://127.0.0.1:1")
        .env("ALL_PROXY", "http://127.0.0.1:1")
        .env("all_proxy", "http://127.0.0.1:1")
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[tokio::test]
async fn https_transport_cannot_downgrade_to_http_even_with_a_private_test_target() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = PinnedTarget {
        url: reqwest::Url::parse("http://media.example/resource").unwrap(),
        host: "media.example".into(),
        addresses: vec![listener.local_addr().unwrap()],
        deadline: Instant::now() + Duration::from_secs(1),
    };
    assert!(matches!(
        network::HttpsTransport.get(&target).await,
        Err(Unavailable::Transport)
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(25), listener.accept())
            .await
            .is_err()
    );
}

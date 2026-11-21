//! Destination resolution and address-pinned clients for outbound requests.
use super::policy::public_ip;
use reqwest::Url;
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

/// More addresses than [`all_public`] accepts, so an oversized answer is
/// observable as a refusal instead of being silently truncated to a pass.
const RESOLVE_LIMIT: usize = 33;
const PUBLIC_LIMIT: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    Unavailable,
}

/// Resolve `url` to socket addresses: IP literals map directly and domains go
/// through the system resolver, keeping at most 33 answers.
pub async fn resolve(url: &Url) -> Result<Vec<SocketAddr>, ResolveError> {
    let port = url
        .port_or_known_default()
        .ok_or(ResolveError::Unavailable)?;
    match url.host().ok_or(ResolveError::Unavailable)? {
        url::Host::Ipv4(ip) => Ok(vec![SocketAddr::new(IpAddr::V4(ip), port)]),
        url::Host::Ipv6(ip) => Ok(vec![SocketAddr::new(IpAddr::V6(ip), port)]),
        url::Host::Domain(host) => Ok(tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| ResolveError::Unavailable)?
            .take(RESOLVE_LIMIT)
            .collect()),
    }
}

/// True only for 1..=32 addresses that are all public; any non-public answer
/// refuses the whole destination so DNS cannot steer one attempt inward.
pub fn all_public(addresses: &[SocketAddr]) -> bool {
    !addresses.is_empty()
        && addresses.len() <= PUBLIC_LIMIT
        && addresses.iter().all(|address| public_ip(address.ip()))
}

/// A client that connects only to the already-checked `addresses` for `host`,
/// ignores proxies and never follows redirects, so the checked answer is the
/// one actually used.
pub fn pinned_client(
    host: &str,
    addresses: &[SocketAddr],
    connect: Duration,
    total: Duration,
) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, addresses)
        .connect_timeout(connect)
        .timeout(total)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addresses(ip: &str, count: usize) -> Vec<SocketAddr> {
        vec![SocketAddr::new(ip.parse().unwrap(), 443); count]
    }

    #[test]
    fn all_public_accepts_one_through_thirty_two_public_addresses() {
        assert!(all_public(&addresses("8.8.8.8", 1)));
        assert!(all_public(&addresses("2606:4700:4700::1111", 32)));
    }

    #[test]
    fn all_public_refuses_empty_oversized_and_mixed_answers() {
        assert!(!all_public(&[]));
        assert!(!all_public(&addresses("8.8.8.8", 33)));
        let mut mixed = addresses("8.8.8.8", 3);
        mixed.push("10.0.0.1:443".parse().unwrap());
        assert!(!all_public(&mixed));
        assert!(!all_public(&addresses("127.0.0.1", 1)));
    }

    #[tokio::test]
    async fn resolve_maps_ip_literals_with_known_or_explicit_ports() {
        let v4 = resolve(&Url::parse("https://8.8.8.8/hook").unwrap())
            .await
            .unwrap();
        assert_eq!(v4, vec!["8.8.8.8:443".parse::<SocketAddr>().unwrap()]);
        let v6 = resolve(&Url::parse("https://[2001:4860::8888]:8443/").unwrap())
            .await
            .unwrap();
        assert_eq!(
            v6,
            vec!["[2001:4860::8888]:8443".parse::<SocketAddr>().unwrap()]
        );
    }

    #[tokio::test]
    async fn resolve_refuses_urls_without_a_host_or_port() {
        for raw in ["mailto:user@example.com", "unknown://host/path"] {
            assert_eq!(
                resolve(&Url::parse(raw).unwrap()).await,
                Err(ResolveError::Unavailable),
                "{raw}"
            );
        }
    }

    #[test]
    fn pinned_client_builds_for_checked_addresses() {
        let client = pinned_client(
            "receiver.example",
            &addresses("8.8.8.8", 2),
            Duration::from_secs(3),
            Duration::from_secs(5),
        );
        assert!(client.is_ok());
    }
}

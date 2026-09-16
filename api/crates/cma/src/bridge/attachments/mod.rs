//! Bounded connector retrieval. Discord text is composed behind explicit opt-in;
//! binary media still needs independent provider authority and a trusted scanner.
use std::net::{IpAddr, SocketAddr};

use sha2::{Digest, Sha256};
use tokio::time::{timeout_at, Instant};
use zeroize::Zeroizing;

pub(crate) mod discord;
mod worker;
pub(crate) use worker::capture;
mod network;
mod types;
use network::{PinnedTarget, Resolver, Transport};
use types::*;

#[async_trait::async_trait]
trait ConnectorAuthority: Send + Sync {
    async fn authorize(&self, request: &FetchRequest) -> Result<FetchGrant, Unavailable>;
    /// Revalidate the same source/bot/owner grant. This cannot extend its deadline.
    async fn revalidate(&self, grant: &FetchGrant) -> Result<(), Unavailable>;
}

/// Fail-closed authority for connectors without a provider-scoped grant.
struct UnavailableAuthority;

#[async_trait::async_trait]
impl ConnectorAuthority for UnavailableAuthority {
    async fn authorize(&self, _: &FetchRequest) -> Result<FetchGrant, Unavailable> {
        Err(Unavailable::Authorization)
    }
    async fn revalidate(&self, _: &FetchGrant) -> Result<(), Unavailable> {
        Err(Unavailable::Authorization)
    }
}

#[derive(Clone, Copy)]
enum ScanOutcome {
    Clean,
    Rejected,
    Unavailable,
}

#[async_trait::async_trait]
trait Scanner: Send + Sync {
    async fn scan(&self, bytes: &[u8], mime: Mime) -> ScanOutcome;
}

struct UnavailableScanner;

#[async_trait::async_trait]
impl Scanner for UnavailableScanner {
    async fn scan(&self, _: &[u8], _: Mime) -> ScanOutcome {
        ScanOutcome::Unavailable
    }
}

async fn retrieve(
    request: &FetchRequest,
    authority: &dyn ConnectorAuthority,
    resolver: &dyn Resolver,
    transport: &dyn Transport,
    scanner: &dyn Scanner,
) -> Result<FetchedAttachment, Unavailable> {
    let deadline = Instant::now() + MAX_DURATION;
    timeout_at(deadline, async {
        let grant = authority.authorize(request).await?;
        if grant.request != *request {
            return Err(Unavailable::Authorization);
        }
        // The attempt is bounded before authorization; a slow grant issuer may
        // shorten but cannot renew that attempt's lifetime.
        let deadline = deadline.min(grant.deadline);
        timeout_at(deadline, async {
            current(deadline)?;
            authority.revalidate(&grant).await?;
            current(deadline)?;
            let addresses = resolver.resolve(&grant.host).await?;
            let target = PinnedTarget::new(&grant, addresses, deadline)?;
            // An already-ready DNS result can win polling over timeout_at.
            // Fence the connection itself with the original attempt's limit.
            current(deadline)?;
            let mut response = transport.get(&target).await?;
            current(deadline)?;
            if !target.addresses.contains(&response.peer) {
                return Err(Unavailable::Address);
            }
            if response.status != 200 || response.encoding.is_some() {
                return Err(Unavailable::Response);
            }
            if response.mime.as_deref() != Some(grant.mime.as_str()) {
                return Err(Unavailable::Mime);
            }
            if response
                .length
                .is_some_and(|n| n > grant.max_bytes as u64 || n == 0)
            {
                return Err(Unavailable::Bounds);
            }
            // Reserve the exact grant limit so geometric Vec growth cannot
            // allocate an accumulator beyond the bounded download budget.
            let mut bytes = Zeroizing::new(Vec::with_capacity(grant.max_bytes));
            while let Some(chunk) = response
                .body
                .next(grant.max_bytes.saturating_sub(bytes.len()))
                .await?
            {
                let chunk = Zeroizing::new(chunk);
                current(deadline)?;
                if chunk.len() > grant.max_bytes.saturating_sub(bytes.len()) {
                    return Err(Unavailable::Bounds);
                }
                bytes.extend_from_slice(&chunk);
            }
            if bytes.is_empty() || response.length.is_some_and(|n| n != bytes.len() as u64) {
                return Err(Unavailable::Bounds);
            }
            if !grant.mime.corroborates(&bytes) {
                return Err(Unavailable::Mime);
            }
            // Strict UTF-8 text uses the ordinary untrusted text-input policy;
            // this is not a malware-clean attestation. Binary files still need
            // an independently supplied scanner and remain unavailable.
            if grant.mime != Mime::Text
                && !matches!(scanner.scan(&bytes, grant.mime).await, ScanOutcome::Clean)
            {
                return Err(Unavailable::Scan);
            }
            current(deadline)?;
            authority.revalidate(&grant).await?;
            current(deadline)?;
            Ok(FetchedAttachment {
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                bytes,
                mime: grant.mime,
                source: grant.request.source,
            })
        })
        .await
        .map_err(|_| Unavailable::Deadline)?
    })
    .await
    .map_err(|_| Unavailable::Deadline)?
}

fn current(deadline: Instant) -> Result<(), Unavailable> {
    if Instant::now() >= deadline {
        Err(Unavailable::Deadline)
    } else {
        Ok(())
    }
}

// Intentionally conservative public-unicast policy. Reject mixed DNS results
// rather than selecting one apparently safe address from an unsafe set.
fn public_address(address: IpAddr) -> bool {
    const DENIED: &[&str] = &[
        "0.0.0.0/8",
        "10.0.0.0/8",
        "100.64.0.0/10",
        "127.0.0.0/8",
        "169.254.0.0/16",
        "172.16.0.0/12",
        "192.0.0.0/24",
        "192.0.2.0/24",
        "192.88.99.0/24",
        "192.168.0.0/16",
        "198.18.0.0/15",
        "198.51.100.0/24",
        "203.0.113.0/24",
        "224.0.0.0/4",
        "240.0.0.0/4",
        "2001::/23",
        "2001:db8::/32",
        "2002::/16",
        "3fff::/20",
    ];
    if let IpAddr::V6(address) = address {
        // Excludes mapped/compatible IPv4, NAT64, local/link-local and multicast.
        if address.segments()[0] & 0xe000 != 0x2000 {
            return false;
        }
    }
    !DENIED.iter().any(|network| {
        network
            .parse::<ipnet::IpNet>()
            .expect("constant network")
            .contains(&address)
    })
}

#[cfg(test)]
pub(crate) mod capture_test_io;
#[cfg(test)]
pub(crate) mod discord_tests;
#[cfg(test)]
mod tests;

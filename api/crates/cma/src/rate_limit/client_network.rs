//! Derive the network budget key from the peer and trusted forwarding headers.

use super::ClientKey;
use crate::config::TrustedProxyConfig;
use axum::{
    extract::{ConnectInfo, Request},
    http::HeaderMap,
};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};

pub(super) fn resolve_client(request: &Request, trusted: &TrustedProxyConfig) -> ClientKey {
    let Some(peer) = request.extensions().get::<ConnectInfo<SocketAddr>>() else {
        return ClientKey::Unidentified;
    };
    let peer_ip = canonical_ip(peer.0.ip());
    let forwarded = trusted
        .cidrs
        .iter()
        .any(|network| network.contains(&peer_ip))
        .then(|| forwarded_client(request.headers(), trusted))
        .flatten();
    ClientKey::Network(normalize_client_ip(forwarded.unwrap_or(peer_ip)))
}

fn forwarded_client(headers: &HeaderMap, trusted: &TrustedProxyConfig) -> Option<IpAddr> {
    let mut tokens = Vec::new();
    let mut found = false;
    for value in headers.get_all("x-forwarded-for") {
        found = true;
        let raw = value.to_str().ok()?;
        for token in raw.split(',') {
            let token = token.trim();
            if token.is_empty() {
                return None;
            }
            tokens.push(token);
        }
    }
    if !found {
        return None;
    }
    let offset = trusted.forwarded_hops.checked_add(1)?;
    let index = tokens.len().checked_sub(offset)?;
    let upstream = tokens[index].parse::<IpAddr>().ok().map(canonical_ip)?;

    if !trusted
        .cloudflare_cidrs
        .iter()
        .any(|network| network.contains(&upstream))
    {
        return Some(upstream);
    }

    // Cloudflare Workers can alter the connecting-client value, while Pseudo
    // IPv4 overwrite mode carries the real address in CF-Connecting-IPv6. Keep
    // the authenticated edge as the key in either case instead of treating the
    // rewritten value as visitor authority.
    if headers.contains_key("cf-worker") || headers.contains_key("cf-connecting-ipv6") {
        return Some(upstream);
    }

    let visitor = index
        .checked_sub(1)
        .and_then(|index| tokens[index].parse::<IpAddr>().ok())
        .map(canonical_ip);
    let connecting = single_ip_header(headers, "cf-connecting-ip");
    Some(match (visitor, connecting) {
        (Some(visitor), Some(connecting))
            if visitor == connecting && !is_cloudflare_pseudo_ipv4(visitor) =>
        {
            visitor
        }
        _ => upstream,
    })
}

fn is_cloudflare_pseudo_ipv4(ip: IpAddr) -> bool {
    matches!(canonical_ip(ip), IpAddr::V4(ip) if ip.octets()[0] >= 240)
}

fn single_ip_header(headers: &HeaderMap, name: &'static str) -> Option<IpAddr> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    if value.is_empty() || value != value.trim() {
        return None;
    }
    value.parse::<IpAddr>().ok().map(canonical_ip)
}

fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(IpAddr::V6(ip), IpAddr::V4),
        ip => ip,
    }
}

pub(super) fn normalize_client_ip(ip: IpAddr) -> IpAddr {
    match canonical_ip(ip) {
        IpAddr::V4(ip) => IpAddr::V4(ip),
        IpAddr::V6(ip) => {
            let prefix = u128::from(ip) & (u128::MAX << 64);
            IpAddr::V6(Ipv6Addr::from(prefix))
        }
    }
}

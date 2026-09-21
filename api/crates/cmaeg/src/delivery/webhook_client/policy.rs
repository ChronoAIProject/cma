use reqwest::Url;
use std::net::IpAddr;
pub(super) fn target(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    match url.host()? {
        url::Host::Ipv4(ip) if !public_ip(IpAddr::V4(ip)) => None,
        url::Host::Ipv6(ip) if !public_ip(IpAddr::V6(ip)) => None,
        _ => Some(url),
    }
}
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || a == 0
                || a >= 240
                || (a == 100 && (64..=127).contains(&b))
                || (a == 192 && b == 0)
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(mapped));
            }
            let words = ip.segments();
            // Only native global unicast; reject translation/tunnelling and documentation ranges too.
            words[0] & 0xe000 == 0x2000
                && words[0] != 0x2002
                && !(words[0] == 0x2001 && (words[1] < 0x200 || words[1] == 0xdb8))
                && words[0] != 0x3fff
        }
    }
}

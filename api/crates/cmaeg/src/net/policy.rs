use reqwest::Url;
use std::net::IpAddr;
pub(crate) fn target(raw: &str) -> Option<Url> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn public(raw: &str) -> bool {
        public_ip(raw.parse().unwrap())
    }

    #[test]
    fn refuses_private_loopback_and_local_ranges() {
        for raw in [
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "127.0.0.1",
            "169.254.169.254",
            "0.0.0.0",
            "224.0.0.1",
            "240.0.0.1",
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
        ] {
            assert!(!public(raw), "{raw}");
        }
    }

    #[test]
    fn refuses_carrier_grade_nat_but_not_its_neighbours() {
        assert!(!public("100.64.0.1"));
        assert!(!public("100.127.255.255"));
        assert!(public("100.63.255.255"));
        assert!(public("100.128.0.1"));
    }

    #[test]
    fn refuses_documentation_and_benchmark_ranges() {
        for raw in [
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "198.18.0.1",
            "2001:db8::1",
        ] {
            assert!(!public(raw), "{raw}");
        }
    }

    #[test]
    fn ipv4_mapped_ipv6_follows_the_mapped_address() {
        assert!(!public("::ffff:10.0.0.1"));
        assert!(!public("::ffff:127.0.0.1"));
        assert!(public("::ffff:8.8.8.8"));
    }

    #[test]
    fn accepts_public_unicast_addresses() {
        for raw in [
            "8.8.8.8",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "2001:4860::8888",
        ] {
            assert!(public(raw), "{raw}");
        }
    }

    #[test]
    fn target_requires_a_plain_https_url_without_a_non_public_literal() {
        assert!(target("https://receiver.example/events").is_some());
        assert!(target("https://8.8.8.8/events").is_some());
        for raw in [
            "http://receiver.example/events",
            "https://user:pass@receiver.example/events",
            "https://receiver.example/events#fragment",
            "https://10.0.0.1/events",
            "https://[::1]/events",
            "not a url",
        ] {
            assert!(target(raw).is_none(), "{raw}");
        }
    }
}

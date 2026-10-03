//! Which addresses a fetch may reach.
//!
//! The rule is **an allowlist of the global unicast space**, not a blocklist of known-private ranges. A
//! blocklist is correct only as long as it is complete, and the IANA special-purpose registries
//! (`docs/research/integrations/web-fetch.md`) keep growing; an address nobody listed would be reachable. Here
//! an IPv6 address is public only inside `2000::/3`, and an IPv4 address is public only outside the blocks the
//! registry marks as not globally reachable, plus multicast and everything reserved from `224.0.0.0` up.
//!
//! Three IPv6 forms **embed an IPv4 address** — IPv4-mapped, NAT64 and 6to4 — and each is judged by the
//! address it embeds. Without that, `::ffff:127.0.0.1` would pass as "an IPv6 address outside the private
//! ranges" while reaching loopback.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Returns whether an address is one a fetch may connect to.
#[must_use]
pub fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_v4(address),
        IpAddr::V6(address) => is_public_v6(address),
    }
}

fn is_public_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    let refused = a == 0 // "this network"
        || a == 10 // RFC 1918
        || a == 127 // loopback
        || (a == 100 && (64..=127).contains(&b)) // shared address space, RFC 6598
        || (a == 169 && b == 254) // link-local, which includes the cloud metadata address
        || (a == 172 && (16..=31).contains(&b)) // RFC 1918
        // The whole /24 is refused although two anycast addresses inside it are globally reachable: nothing
        // here needs them, and refusing a block is the cheaper error.
        || (a == 192 && b == 0 && (c == 0 || c == 2)) // IETF protocol assignments, TEST-NET-1
        || (a == 192 && b == 88 && c == 99) // deprecated 6to4 relay anycast
        || (a == 192 && b == 168) // RFC 1918
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || (a == 198 && b == 51 && c == 100) // TEST-NET-2
        || (a == 203 && b == 0 && c == 113) // TEST-NET-3
        || a >= 224; // multicast, reserved, and the limited broadcast address
    !refused
}

fn is_public_v6(address: Ipv6Addr) -> bool {
    if let Some(embedded) = address.to_ipv4_mapped() {
        return is_public_v4(embedded);
    }
    let segments = address.segments();
    // NAT64, 64:ff9b::/96: the low 32 bits are an IPv4 address the translator will connect to.
    if segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2..6].iter().all(|s| *s == 0) {
        return is_public_v4(embed(segments[6], segments[7]));
    }
    // 6to4, 2002::/16: bits 16..48 are an IPv4 address.
    if segments[0] == 0x2002 {
        return is_public_v4(embed(segments[1], segments[2]));
    }
    // Global unicast only. This one test removes loopback, unspecified, unique-local, link-local, multicast,
    // the discard prefix, and every block nobody has allocated yet.
    if segments[0] & 0xe000 != 0x2000 {
        return false;
    }
    // The two documentation blocks and the IETF protocol-assignment block (which includes Teredo).
    let refused = (segments[0] == 0x2001 && (segments[1] < 0x0200 || segments[1] == 0x0db8))
        || (segments[0] == 0x3fff && segments[1] < 0x1000); // RFC 9637
    !refused
}

fn embed(high: u16, low: u16) -> Ipv4Addr {
    let [a, b] = high.to_be_bytes();
    let [c, d] = low.to_be_bytes();
    Ipv4Addr::new(a, b, c, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse()
            .unwrap_or_else(|error| panic!("{text}: {error}"))
    }

    #[test]
    fn every_registry_block_that_is_not_globally_reachable_is_refused() {
        for text in [
            "0.0.0.0",
            "10.1.2.3",
            "100.64.0.1",
            "100.127.255.255",
            "127.0.0.1",
            "127.255.255.254",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.1",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "ff02::1",
            "100::1",
            "64:ff9b:1::1",
            "2001::1",
            "2001:1::1",
            "2001:db8::1",
            "3fff::1",
            "5f00::1",
        ] {
            assert!(!is_public(ip(text)), "{text} must be refused");
        }
    }

    #[test]
    fn ordinary_public_addresses_are_allowed() {
        for text in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "100.63.255.255",
            "100.128.0.1",
            "172.15.255.255",
            "172.32.0.1",
            "192.0.1.1",
            "192.169.0.1",
            "198.17.255.255",
            "198.20.0.1",
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
        ] {
            assert!(is_public(ip(text)), "{text} must be allowed");
        }
    }

    #[test]
    fn an_ipv6_form_that_embeds_ipv4_is_judged_by_what_it_embeds() {
        // Each of these is "an IPv6 address that is in no private IPv6 range", and each reaches a private IPv4
        // host. A guard that judged only the outer address would pass all of them.
        for text in [
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "::ffff:10.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::a9fe:a9fe",
            "2002:7f00:1::",
            "2002:c0a8:101::1",
        ] {
            assert!(!is_public(ip(text)), "{text} must be refused");
        }
        for text in ["::ffff:8.8.8.8", "64:ff9b::808:808", "2002:0808:0808::1"] {
            assert!(is_public(ip(text)), "{text} embeds a public address");
        }
    }

    #[test]
    fn an_unallocated_ipv6_block_is_refused_because_the_rule_is_an_allowlist() {
        // Nothing lists `4000::/2` as special. It is refused because it is not global unicast.
        assert!(!is_public(ip("4000::1")));
        assert!(!is_public(ip("8000::1")));
        assert!(!is_public(ip("e000::1")));
    }
}

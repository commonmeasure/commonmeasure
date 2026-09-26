//! The privacy floor's one classifier: whether a URL names a local or
//! private address. The harness applies it to what may be recorded and
//! mediated; the runner applies it to where a supplier's source came from.
//! One home, so a URL is private or public by one rule everywhere.
//!
//! The rule for an address: it is private when the IANA IPv4 and IPv6
//! Special-Purpose Address Registries mark it not globally reachable, and an
//! IPv6 address that embeds an IPv4 address is judged as that IPv4 address.
//! The registries are read as the standard library's unstable
//! `Ipv4Addr::is_global` and `Ipv6Addr::is_global` read them (Rust 1.98),
//! mirrored here because those are not stable. Where this differs from
//! `is_global`, it judges an embedded IPv4 address as that address, stricter
//! or looser as that address is, and is otherwise stricter, each for a stated
//! reason:
//!
//! - The embedded forms are IPv4-mapped `::ffff:0:0/96`, IPv4-compatible
//!   `::/96`, NAT64 `64:ff9b::/96` (RFC 6052) and 6to4 `2002::/16`, which
//!   carries the IPv4 address in bits 16 to 47 (RFC 3056). `is_global` calls
//!   all of mapped and 6to4 non-global and all of IPv4-compatible and NAT64
//!   global; judging the IPv4 address instead keeps `64:ff9b::10.0.0.5`,
//!   which a NAT64 gateway delivers to `10.0.0.5`, out, and
//!   `64:ff9b::8.8.8.8` in.
//! - The local-use NAT64 prefix `64:ff9b:1::/48` (RFC 8215) is private whole:
//!   where the IPv4 address sits in it depends on a prefix length only the
//!   network's operator knows.
//! - Multicast, in both families, is private: never a unicast destination a
//!   fetch can name. So is deprecated IPv6 site-local `fec0::/10` (RFC 3879).
//! - Teredo `2001::/32` is private, as part of `2001::/23`; the client
//!   address it obfuscates into its last 32 bits is not decoded.
//!
//! The documentation ranges (`192.0.2.0/24`, `198.51.100.0/24`,
//! `203.0.113.0/24`, `2001:db8::/32`, `3fff::/20`) follow the registry and are
//! private. `198.18.0.0/15`, benchmarking in the registry, is also the range
//! fake-IP proxies (Clash, Surge, sing-box) answer every name with, so on a
//! machine behind one every name resolves to a private address here; the
//! mediated path says so in its refusal ([`is_fake_ip_range`]).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Whether a parsed URL points at a local or private address: `file://`,
/// a host that is none, `localhost` and the `.localhost`, `.local` and
/// `.internal` suffixes, and every address the module's rule calls private.
/// A domain is judged as the name it is, whatever it resolves to:
/// `10.example.com` is public.
pub fn is_private(url: &url::Url) -> bool {
    if url.scheme() == "file" {
        return true;
    }
    // The parsed host, not `host_str`: an IPv6 host string arrives bracketed
    // (`[::1]`), and matching the parsed address is what lets whole ranges be
    // judged — all of 127.0.0.0/8, not one literal — while a domain that
    // merely starts with digits (`10.example.com`) stays the public name it
    // is.
    match url.host() {
        None => true,
        Some(url::Host::Domain(domain)) => {
            // The host as admission reads it: `host.internal.` is the name
            // `host.internal` is, and read as written it would be public.
            let domain = crate::normalised_host(domain);
            domain == "localhost"
                || domain.ends_with(".localhost")
                || domain.ends_with(".local")
                || domain.ends_with(".internal")
        }
        Some(url::Host::Ipv4(address)) => is_private_ip(IpAddr::V4(address)),
        Some(url::Host::Ipv6(address)) => is_private_ip(IpAddr::V6(address)),
    }
}

/// [`is_private`] over a URL as spelled. A URL that does not parse is
/// private: nothing that cannot be named can be public.
pub fn is_private_address(raw: &str) -> bool {
    url::Url::parse(raw).map_or(true, |url| is_private(&url))
}

/// Whether an address is private by the module's rule.
pub fn is_private_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => !is_global_v4(v4),
        IpAddr::V6(v6) => !is_global_v6(v6),
    }
}

/// Whether an address is in `198.18.0.0/15`, directly or embedded in IPv6:
/// the range fake-IP proxies answer every name with. Private by the rule;
/// named separately so that a refusal can say why every name is refused.
pub fn is_fake_ip_range(address: IpAddr) -> bool {
    as_ipv4(address).is_some_and(|v4| matches!(v4.octets(), [198, 18 | 19, _, _]))
}

/// Whether an address is in `100.64.0.0/10` (RFC 6598), directly or
/// embedded in IPv6: shared address space, where Tailscale tailnets and
/// carrier-grade NAT live. Private by the rule; named separately so that a
/// refusal can say how to allow one host there.
pub fn is_shared_range(address: IpAddr) -> bool {
    as_ipv4(address).is_some_and(|v4| matches!(v4.octets(), [100, 64..=127, _, _]))
}

fn as_ipv4(address: IpAddr) -> Option<Ipv4Addr> {
    match address {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(v6) => embedded_ipv4(v6),
    }
}

/// The IPv4 address an IPv6 address carries, in the forms where its position
/// is fixed by the prefix.
fn embedded_ipv4(address: Ipv6Addr) -> Option<Ipv4Addr> {
    let v4 = |high: u16, low: u16| Ipv4Addr::from((u32::from(high) << 16) | u32::from(low));
    match address.segments() {
        // IPv4-mapped `::ffff:0:0/96` and IPv4-compatible `::/96`. The
        // latter holds `::` and `::1`, which come out as `0.0.0.0` and
        // `0.0.0.1`, both private as `0.0.0.0/8`.
        [0, 0, 0, 0, 0, 0xffff | 0, high, low]
        // NAT64 well-known prefix `64:ff9b::/96`.
        | [0x64, 0xff9b, 0, 0, 0, 0, high, low]
        // 6to4 `2002::/16`, bits 16 to 47.
        | [0x2002, high, low, ..] => Some(v4(high, low)),
        _ => None,
    }
}

/// `Ipv4Addr::is_global` (Rust 1.98), plus multicast.
fn is_global_v4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    !(octets[0] == 0 // "This network", 0.0.0.0/8
        || address.is_private()
        || matches!(octets, [100, 64..=127, _, _]) // shared, 100.64.0.0/10
        || address.is_loopback()
        || address.is_link_local()
        // IETF protocol assignments, 192.0.0.0/24; .9 and .10 are
        // globally reachable anycast.
        || (matches!(octets, [192, 0, 0, _]) && octets[3] != 9 && octets[3] != 10)
        || address.is_documentation()
        || matches!(octets, [198, 18 | 19, _, _]) // benchmarking, 198.18.0.0/15
        || octets[0] >= 240 // reserved 240.0.0.0/4, broadcast among it
        || address.is_multicast())
}

/// `Ipv6Addr::is_global` (Rust 1.98) with the embedded forms judged as their
/// IPv4 address, plus multicast and site-local.
fn is_global_v6(address: Ipv6Addr) -> bool {
    if let Some(v4) = embedded_ipv4(address) {
        return is_global_v4(v4);
    }
    let segments = address.segments();
    let bits = u128::from(address);
    !(matches!(segments, [0x64, 0xff9b, 1, ..]) // local-use NAT64, 64:ff9b:1::/48
        || matches!(segments, [0x100, 0, 0, 0, ..]) // discard-only, 100::/64
        // IETF protocol assignments, 2001::/23 (Teredo and benchmarking
        // among it), less the ranges the registry marks globally reachable.
        || (matches!(segments, [0x2001, second, ..] if second < 0x200)
            && !(bits == 0x2001_0001_0000_0000_0000_0000_0000_0001 // PCP anycast
                || bits == 0x2001_0001_0000_0000_0000_0000_0000_0002 // TURN anycast
                || matches!(segments, [0x2001, 3, ..]) // AMT
                || matches!(segments, [0x2001, 4, 0x112, ..]) // AS112-v6
                || matches!(segments, [0x2001, 0x20..=0x3f, ..]))) // ORCHIDv2, DETs
        || matches!(segments, [0x2001, 0xdb8, ..] | [0x3fff, 0..=0x0fff, ..]) // documentation
        || segments[0] == 0x5f00 // SRv6 SIDs, 5f00::/16
        || address.is_unique_local()
        || address.is_unicast_link_local()
        || segments[0] & 0xffc0 == 0xfec0 // site-local, fec0::/10
        || address.is_multicast())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_and_local_addresses_are_private_and_public_names_are_not() {
        for private in [
            "http://localhost:3000/x",
            "http://127.0.0.2/secret",
            "http://10.1.2.3/",
            "http://[::1]:8080/admin",
            "http://[::ffff:192.168.0.1]/",
            "file:///home/op/notes.md",
            "http://rag.corp.internal/kb",
            "https://host.internal./a",
            "http://LOCALHOST.:3000/x",
            "http://printer.local../",
            "not a url",
        ] {
            assert!(is_private_address(private), "{private}");
        }
        for public in [
            "https://www.gov.uk/",
            "https://www.gov.uk./",
            "https://internal.example./",
            "http://10.example.com/",
            "https://8.8.8.8/",
        ] {
            assert!(!is_private_address(public), "{public}");
        }
    }

    fn all_private(urls: &[&str]) {
        for url in urls {
            assert!(is_private_address(url), "{url} should be private");
        }
    }

    fn all_public(urls: &[&str]) {
        for url in urls {
            assert!(!is_private_address(url), "{url} should be public");
        }
    }

    #[test]
    fn this_network_is_private_beyond_the_unspecified_address() {
        all_private(&["http://0.0.0.1/", "http://0.255.255.255/"]);
    }

    /// RFC 6598 shared address space: Tailscale tailnets, carrier-grade NAT,
    /// EKS custom networking, and Alibaba Cloud's metadata service.
    #[test]
    fn shared_address_space_is_private() {
        all_private(&[
            "http://100.64.0.1/",
            "http://100.100.100.200/latest/meta-data/",
            "http://100.101.102.103/",
            "http://100.127.255.254/",
        ]);
        all_public(&[
            "http://100.128.0.1/",
            "http://100.63.255.255/",
            "http://99.255.255.255/",
        ]);
    }

    /// 192.0.0.0/24 is IETF protocol assignments, with the two anycast
    /// addresses the registry marks globally reachable.
    #[test]
    fn ietf_protocol_assignments_are_private_except_the_global_anycast_pair() {
        all_private(&[
            "http://192.0.0.1/",
            "http://192.0.0.8/",
            "http://192.0.0.170/",
        ]);
        all_public(&[
            "http://192.0.0.9/",
            "http://192.0.0.10/",
            "http://192.0.1.1/",
        ]);
    }

    /// Also the range fake-IP proxies answer every name with; that is the
    /// resolved-address floor's concern, and the range stays private here.
    #[test]
    fn benchmarking_space_is_private() {
        all_private(&["http://198.18.0.1/", "http://198.19.255.254/"]);
        all_public(&["http://198.20.0.1/", "http://198.17.255.255/"]);
    }

    #[test]
    fn reserved_space_is_private() {
        all_private(&[
            "http://240.0.0.1/",
            "http://254.1.2.3/",
            "http://255.255.255.254/",
        ]);
    }

    #[test]
    fn documentation_ranges_are_private() {
        all_private(&[
            "http://192.0.2.1/",
            "http://198.51.100.7/",
            "http://203.0.113.9/",
            "http://[2001:db8::1]/",
            "http://[3fff::1]/",
        ]);
    }

    #[test]
    fn multicast_is_private_in_both_families() {
        all_private(&[
            "http://224.0.0.1/",
            "http://239.255.255.250/",
            "http://[ff02::1]/",
            "http://[ff0e::1]/",
        ]);
    }

    /// `::a.b.c.d`, deprecated but still parsed, carries the IPv4 address in
    /// its last 32 bits.
    #[test]
    fn ipv4_compatible_ipv6_is_judged_as_its_ipv4_address() {
        all_private(&[
            "http://[::127.0.0.1]/",
            "http://[::10.0.0.5]/",
            "http://[::100.64.0.1]/",
        ]);
        all_public(&["http://[::8.8.8.8]/"]);
    }

    /// The NAT64 well-known prefix translates to the IPv4 address in its last
    /// 32 bits on a network with a gateway (RFC 6052).
    #[test]
    fn nat64_well_known_prefix_is_judged_as_its_ipv4_address() {
        all_private(&[
            "http://[64:ff9b::a00:5]/",
            "http://[64:ff9b::127.0.0.1]/",
            "http://[64:ff9b::169.254.169.254]/",
            "http://[64:ff9b::100.100.100.200]/",
        ]);
        all_public(&["http://[64:ff9b::808:808]/"]);
    }

    /// The local-use NAT64 prefix (RFC 8215) is not globally reachable, and
    /// where the IPv4 address sits in it depends on the operator's prefix
    /// length, so the whole /48 is private.
    #[test]
    fn local_use_nat64_prefix_is_private() {
        all_private(&["http://[64:ff9b:1::a00:5]/", "http://[64:ff9b:1::808:808]/"]);
    }

    /// 6to4 carries the IPv4 address in bits 16 to 47 (RFC 3056). The last
    /// two probes of each list pin that offset: read one segment later, or
    /// two, each of those would be judged the other way.
    #[test]
    fn six_to_four_is_judged_as_its_ipv4_address() {
        all_private(&[
            "http://[2002:a00:5::]/",
            "http://[2002:a9fe:a9fe::]/",
            "http://[2002:7f00:1::]/",
            "http://[2002:6440:1::1]/",
            "http://[2002:a00:808:808::1]/",
            "http://[2002:a00:5:808:808::1]/",
        ]);
        all_public(&[
            "http://[2002:808:808::]/",
            "http://[2002:808:a00:5::1]/",
            "http://[2002:808:808:a00:5::1]/",
        ]);
    }

    #[test]
    fn discard_only_prefix_is_private() {
        all_private(&["http://[100::1]/", "http://[100::ffff:ffff:ffff:ffff]/"]);
    }

    /// 2001::/23, Teredo `2001::/32` and benchmarking `2001:2::/48` among it,
    /// with the anycast and AMT, AS112 and ORCHIDv2 exceptions the registry
    /// marks globally reachable. The client address Teredo obfuscates into
    /// its last 32 bits is not decoded: the whole prefix is private.
    #[test]
    fn ietf_protocol_assignments_v6_and_teredo_are_private_with_the_registry_exceptions() {
        all_private(&[
            "http://[2001::1]/",
            "http://[2001:0:4136:e378:8000:63bf:3fff:fdd2]/",
            "http://[2001:2::1]/",
            "http://[2001:1ff::1]/",
        ]);
        all_public(&[
            "http://[2001:1::1]/",
            "http://[2001:1::2]/",
            "http://[2001:3::1]/",
            "http://[2001:4:112::1]/",
            "http://[2001:20::1]/",
            "http://[2001:200::1]/",
        ]);
    }

    #[test]
    fn segment_routing_sids_are_private() {
        all_private(&["http://[5f00::1]/"]);
    }

    /// Deprecated site-local (RFC 3879), outside the registry but never a
    /// global address.
    #[test]
    fn site_local_ipv6_is_private() {
        all_private(&["http://[fec0::1]/", "http://[feff::1]/"]);
    }

    #[test]
    fn ipv4_mapped_ipv6_follows_the_wider_ipv4_rule() {
        all_private(&["http://[::ffff:100.64.0.1]/", "http://[::ffff:198.18.0.1]/"]);
        all_public(&["http://[::ffff:8.8.8.8]/"]);
    }

    #[test]
    fn public_addresses_stay_public() {
        all_public(&[
            "http://1.1.1.1/",
            "http://[2606:4700:4700::1111]/",
            "http://[2a00:1450:4009:81f::200e]/",
            "http://223.255.255.255/",
        ]);
    }
}

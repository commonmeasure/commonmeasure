//! The privacy floor's one classifier: whether a URL names a local or
//! private address. The harness applies it to what may be recorded and
//! mediated; the runner applies it to where a supplier's source came from.
//! One home, so a URL is private or public by one rule everywhere.

/// Whether a parsed URL points at a local or private address: `file://`,
/// a host that is none, `localhost` and the `.localhost`, `.local` and
/// `.internal` suffixes, and every loopback, private, link-local,
/// unspecified or broadcast address.
pub fn is_private(url: &url::Url) -> bool {
    if url.scheme() == "file" {
        return true;
    }
    // The parsed host, not `host_str`: an IPv6 host string arrives bracketed
    // (`[::1]`), and matching the parsed address is what lets the standard
    // library answer for whole ranges — all of 127.0.0.0/8, not one literal —
    // while a domain that merely starts with digits (`10.example.com`) stays
    // the public name it is.
    match url.host() {
        None => true,
        Some(url::Host::Domain(domain)) => {
            let domain = domain.to_lowercase();
            domain == "localhost"
                || domain.ends_with(".localhost")
                || domain.ends_with(".local")
                || domain.ends_with(".internal")
        }
        Some(url::Host::Ipv4(address)) => is_private_v4(address),
        Some(url::Host::Ipv6(address)) => {
            // An IPv4 address carried inside IPv6 is judged as the IPv4
            // address it names.
            if let Some(mapped) = address.to_ipv4_mapped() {
                return is_private_v4(mapped);
            }
            address.is_loopback()
                || address.is_unspecified()
                || address.is_unique_local()
                || address.is_unicast_link_local()
        }
    }
}

/// [`is_private`] over a URL as spelled. A URL that does not parse is
/// private: nothing that cannot be named can be public.
pub fn is_private_address(raw: &str) -> bool {
    url::Url::parse(raw).map_or(true, |url| is_private(&url))
}

fn is_private_v4(address: std::net::Ipv4Addr) -> bool {
    address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_broadcast()
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
            "not a url",
        ] {
            assert!(is_private_address(private), "{private}");
        }
        for public in [
            "https://www.gov.uk/",
            "http://10.example.com/",
            "https://8.8.8.8/",
        ] {
            assert!(!is_private_address(public), "{public}");
        }
    }
}

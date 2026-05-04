use std::net::{IpAddr, SocketAddr};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// DNS resolver that blocks connections to private and reserved IP addresses.
///
/// Using this as a custom resolver with `reqwest::Client::builder().dns_resolver()`
/// closes the TOCTOU window that exists when doing a pre-flight IP check before
/// opening a connection: without this, a DNS rebinding attack can return a public
/// IP for the pre-flight check and then switch to a private IP for the actual TCP
/// connect. Because `SafeResolver` filters addresses inside the resolver itself,
/// every connection attempt is validated against the same resolved addresses that
/// reqwest actually uses.
pub(super) struct SafeResolver;

impl Resolve for SafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            // Bypass IP classification when the test-support escape hatch is active.
            // This allows integration tests to talk to a mock server on 127.0.0.1
            // without disabling SSRF protection in the production code path.
            #[cfg(feature = "test-support")]
            if std::env::var_os("LORE_TEST_ALLOW_LOOPBACK").is_some() {
                let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0u16))
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                    .collect();
                if addrs.is_empty() {
                    return Err("DNS lookup returned no addresses".into());
                }
                return Ok(Box::new(addrs.into_iter()) as Addrs);
            }

            // Resolve via the OS resolver (same as reqwest's default GaiResolver).
            let lookup = tokio::net::lookup_host((host.as_str(), 0u16))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            let safe: Vec<SocketAddr> = lookup.filter(|sa| !is_private_ip(sa.ip())).collect();
            if safe.is_empty() {
                return Err("SSRF blocked: host resolves to private/reserved address".into());
            }
            Ok(Box::new(safe.into_iter()) as Addrs)
        })
    }
}

/// 6to4: bits 16-47 of the IPv6 address encode the original IPv4 address.
fn extract_6to4_v4(segments: &[u16; 8]) -> std::net::Ipv4Addr {
    std::net::Ipv4Addr::new(
        (segments[1] >> 8) as u8,
        segments[1] as u8,
        (segments[2] >> 8) as u8,
        segments[2] as u8,
    )
}

/// Check if an IP address is non-routable (private, reserved, loopback, etc.)
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_private()
                || v4.is_link_local()
                || o[0] == 0 // 0.0.0.0/8
                || (o[0] == 100 && (o[1] & 0xC0) == 64) // 100.64.0.0/10 (CGNAT)
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24
                || (o[0] == 192 && o[1] == 0 && o[2] == 2) // 192.0.2.0/24 (TEST-NET-1)
                || (o[0] == 198 && o[1] == 51 && o[2] == 100) // 198.51.100.0/24 (TEST-NET-2)
                || (o[0] == 203 && o[1] == 0 && o[2] == 113) // 203.0.113.0/24 (TEST-NET-3)
                || (o[0] == 198 && (o[1] & 0xFE) == 18) // 198.18.0.0/15 (benchmarking)
                || o[0] >= 240 // 240.0.0.0/4 (reserved)
        }
        IpAddr::V6(v6) => {
            // IPv4-mapped IPv6 (::ffff:0:0/96) -- check the embedded IPv4 address.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            let segments = v6.segments();
            v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                || (segments[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
                || (segments[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
                // Teredo tunneling: the full Teredo range is 2001:0000::/32, so
                // blocking segments[1] == 0x0000 covers all Teredo addresses and
                // prevents encoded private IPv4 from bypassing the SSRF check.
                || (segments[0] == 0x2001 && segments[1] == 0x0000)
                // 6to4 (2002::/16) -- encodes IPv4 in bits 16-47; check embedded address
                || (segments[0] == 0x2002
                    && is_private_ip(IpAddr::V4(extract_6to4_v4(&segments))))
                // NAT64 well-known (64:ff9b::/96) and local-use (64:ff9b:1::/48, RFC 8215)
                || (segments[0] == 0x0064 && segments[1] == 0xff9b)
                // Documentation range (2001:db8::/32) -- non-routable
                || (segments[0] == 0x2001 && segments[1] == 0x0db8)
                // Benchmarking (2001:2::/48) -- non-routable
                || (segments[0] == 0x2001 && segments[1] == 0x0002 && segments[2] == 0x0000)
                // ORCHIDv2 (2001:20::/28) -- overlay routable cryptographic hash IDs
                || (segments[0] == 0x2001 && (segments[1] & 0xfff0) == 0x0020)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_classification() {
        // Private/reserved addresses must be blocked
        for addr in [
            "10.0.0.1",
            "172.16.0.1",
            "192.168.0.1",
            "127.0.0.1",
            "0.0.0.0",
            "0.255.255.255",
            "169.254.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "192.0.0.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(
                is_private_ip(addr.parse::<IpAddr>().unwrap()),
                "{addr} should be blocked"
            );
        }

        // Public IPv4 addresses must be allowed
        for addr in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "151.101.1.140",
            "172.32.0.1",
            "192.169.0.1",
        ] {
            assert!(
                !is_private_ip(addr.parse::<IpAddr>().unwrap()),
                "{addr} should be allowed"
            );
        }

        // Private/reserved IPv6 addresses must be blocked
        for addr in ["::1", "::", "fe80::1", "fc00::1", "fd00::1", "2001:db8::1"] {
            assert!(
                is_private_ip(addr.parse::<IpAddr>().unwrap()),
                "{addr} should be blocked"
            );
        }

        // Public IPv6 addresses must be allowed
        for addr in ["2607:f8b0:4004:800::200e", "2606:4700:4700::1111"] {
            assert!(
                !is_private_ip(addr.parse::<IpAddr>().unwrap()),
                "{addr} should be allowed"
            );
        }

        // IPv4-mapped IPv6 (::ffff:0:0/96)
        for addr in [
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "::ffff:192.168.1.1",
        ] {
            assert!(
                is_private_ip(addr.parse::<IpAddr>().unwrap()),
                "{addr} (IPv4-mapped) should be blocked"
            );
        }
        // Public IPv4 via mapped address should be allowed
        assert!(
            !is_private_ip("::ffff:8.8.8.8".parse::<IpAddr>().unwrap()),
            "::ffff:8.8.8.8 (public IPv4-mapped) should be allowed"
        );

        // 6to4 (2002::/16) with embedded private IPv4
        for addr in [
            "2002:7f00:0001::", // embeds 127.0.0.1
            "2002:0a00:0001::", // embeds 10.0.0.1
            "2002:a9fe:a9fe::", // embeds 169.254.169.254
            "2002:c0a8:0101::", // embeds 192.168.1.1
        ] {
            assert!(
                is_private_ip(addr.parse::<IpAddr>().unwrap()),
                "{addr} (6to4 with private IPv4) should be blocked"
            );
        }
        // 6to4 with public IPv4 should be allowed
        assert!(
            !is_private_ip("2002:0808:0808::".parse::<IpAddr>().unwrap()),
            "2002:0808:0808:: (6to4 with 8.8.8.8) should be allowed"
        );

        // Teredo (2001:0000::/32) -- blocked
        assert!(
            is_private_ip(
                "2001:0000:4136:e378:8000:63bf:3fff:fdd2"
                    .parse::<IpAddr>()
                    .unwrap()
            ),
            "Teredo address should be blocked"
        );
        // Outside Teredo /32 -- not blocked by this rule
        assert!(
            !is_private_ip("2001:0001::".parse::<IpAddr>().unwrap()),
            "2001:0001:: is outside Teredo /32 and should not be blocked"
        );

        // NAT64 well-known prefix (64:ff9b::/96) -- blocked
        assert!(
            is_private_ip("64:ff9b::1".parse::<IpAddr>().unwrap()),
            "NAT64 address should be blocked"
        );

        // NAT64 local-use prefix (64:ff9b:1::/48, RFC 8215) -- blocked
        assert!(
            is_private_ip("64:ff9b:1::1".parse::<IpAddr>().unwrap()),
            "NAT64 local-use address should be blocked"
        );

        // ORCHIDv2 (2001:0020::/28) -- blocked
        assert!(
            is_private_ip("2001:0020::1".parse::<IpAddr>().unwrap()),
            "ORCHIDv2 address should be blocked"
        );
        assert!(
            is_private_ip(
                "2001:002f:ffff:ffff:ffff:ffff:ffff:ffff"
                    .parse::<IpAddr>()
                    .unwrap()
            ),
            "ORCHIDv2 boundary address should be blocked"
        );
        // Outside ORCHIDv2 /28 -- not blocked by this rule
        assert!(
            !is_private_ip("2001:0030::".parse::<IpAddr>().unwrap()),
            "2001:0030:: is outside ORCHIDv2 /28 and should not be blocked"
        );
    }
}

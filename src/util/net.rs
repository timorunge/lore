//! Network utility helpers.

/// Return `true` if `host` is a loopback address or hostname.
pub fn is_loopback(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::is_loopback;

    #[test]
    fn is_loopback_cases() {
        let cases: &[(&str, bool)] = &[
            ("localhost", true),
            ("127.0.0.1", true),
            ("::1", true),
            ("0.0.0.0", false),
            ("192.168.1.1", false),
        ];
        for &(host, expected) in cases {
            assert_eq!(
                is_loopback(host),
                expected,
                "is_loopback({host:?}) should be {expected}"
            );
        }
    }
}

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use url::Url;

/// Rejects loopback, private, link-local, multicast and reserved address
/// literals. Hostnames are checked after DNS resolution in [`ensure_public_url`].
pub fn is_forbidden_host(host: &str) -> bool {
    let host = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() {
        return true;
    }
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".lan")
        || host.ends_with(".home.arpa")
    {
        return true;
    }
    host.parse::<IpAddr>()
        .map(ip_is_forbidden)
        .unwrap_or(false)
}

/// Resolves a URL host and rejects any address that is not publicly routable.
/// When `allow_private` is true, both the literal and DNS checks are skipped.
pub async fn ensure_public_url(url: &Url, allow_private: bool) -> Result<(), String> {
    if allow_private {
        return Ok(());
    }
    let host = url.host_str().ok_or_else(|| "URL has no host".to_string())?;
    if is_forbidden_host(host) {
        return Err(format!("host {host} is private, loopback or local"));
    }
    if host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .is_ok()
    {
        return Ok(());
    }

    let port = url.port_or_known_default().unwrap_or(80);
    let lookup = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::lookup_host((host, port)),
    )
    .await;
    let addresses: Vec<_> = match lookup {
        Ok(Ok(addresses)) => addresses.collect(),
        Ok(Err(error)) => return Err(format!("DNS lookup failed for {host}: {error}")),
        Err(_) => return Err(format!("DNS lookup timed out for {host}")),
    };
    if addresses.is_empty() {
        return Err(format!("DNS lookup returned no addresses for {host}"));
    }
    for address in addresses {
        if ip_is_forbidden(address.ip()) {
            return Err(format!(
                "host {host} resolves to a private or local address ({})",
                address.ip()
            ));
        }
    }
    Ok(())
}

fn ip_is_forbidden(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ipv4_is_forbidden(ip),
        IpAddr::V6(ip) => ipv6_is_forbidden(ip),
    }
}

fn ipv4_is_forbidden(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_unspecified()
        || octets[0] == 0
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        // 198.18.0.0/15 is deliberately not blocked: proxy tools such as Clash
        // use it as a fake-IP range for proxied public hostnames.
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
        || octets[0] >= 240
}

fn ipv6_is_forbidden(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    if let Some(ipv4) = ip.to_ipv4_mapped() {
        return ipv4_is_forbidden(ipv4);
    }
    let segments = ip.segments();
    (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_loopback_and_private_literals() {
        for host in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
            "localhost",
            "foo.localhost",
            "printer.local",
            "service.internal",
        ] {
            assert!(is_forbidden_host(host), "{host} should be blocked");
        }
    }

    #[test]
    fn allows_public_literals_and_names() {
        for host in [
            "8.8.8.8",
            "1.1.1.1",
            "198.18.1.155", // Clash fake-IP range
            "2606:4700:4700::1111",
            "example.com",
        ] {
            assert!(!is_forbidden_host(host), "{host} should be allowed");
        }
    }
}

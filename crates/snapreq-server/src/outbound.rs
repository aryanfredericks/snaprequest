//! The outbound request policy (SSRF protection): which destinations requests executed
//! by the server may reach.
//!
//! Checked by the engine on every resolved address before connecting, so DNS names that
//! point at internal addresses, pinned hosts and redirects are all covered.
//!
//! Order: a `deny` rule always blocks; an `allow` rule match always permits; then
//! `block_metadata` and `block_private` apply; finally, a non-empty `allow` list blocks
//! everything it does not match.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::Deserialize;
use snapreq_core::ConnectPolicy;

/// Cloud metadata endpoints (AWS, GCP, Azure, Alibaba, Oracle, ECS task metadata).
const METADATA_IPS: [IpAddr; 4] = [
    IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
    IpAddr::V4(Ipv4Addr::new(169, 254, 170, 2)),
    IpAddr::V4(Ipv4Addr::new(100, 100, 100, 200)),
    IpAddr::V6(Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254)),
];
const METADATA_HOSTS: [&str; 3] = ["metadata.google.internal", "metadata.goog", "metadata"];

/// A destination rule: `10.0.0.0/8`, `192.168.1.5`, `::1`, `api.example.com` or
/// `*.example.com` (the domain and every subdomain).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    Net { ip: IpAddr, prefix: u8 },
    Host(String),
    Suffix(String),
}

impl std::str::FromStr for Rule {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty rule".into());
        }
        if let Some((ip, prefix)) = s.split_once('/') {
            let ip: IpAddr = ip
                .trim_matches(['[', ']'])
                .parse()
                .map_err(|_| format!("invalid network `{s}`"))?;
            let max = if ip.is_ipv4() { 32 } else { 128 };
            let prefix: u8 = prefix
                .parse()
                .ok()
                .filter(|p| *p <= max)
                .ok_or_else(|| format!("invalid prefix length in `{s}` (0–{max})"))?;
            return Ok(Self::Net {
                ip: canonical(ip),
                prefix,
            });
        }
        if let Ok(ip) = s.trim_matches(['[', ']']).parse::<IpAddr>() {
            let ip = canonical(ip);
            return Ok(Self::Net {
                ip,
                prefix: if ip.is_ipv4() { 32 } else { 128 },
            });
        }
        let host = s.to_ascii_lowercase();
        if let Some(domain) = host.strip_prefix("*.") {
            return Ok(Self::Suffix(domain.trim_end_matches('.').to_owned()));
        }
        if host.contains(['/', ':', ' ', '*']) {
            return Err(format!(
                "invalid rule `{s}` (a CIDR, an IP, a host or `*.domain`)"
            ));
        }
        Ok(Self::Host(host.trim_end_matches('.').to_owned()))
    }
}

impl std::fmt::Display for Rule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Net { ip, prefix } => write!(f, "{ip}/{prefix}"),
            Self::Host(h) => f.write_str(h),
            Self::Suffix(d) => write!(f, "*.{d}"),
        }
    }
}

impl Rule {
    fn matches(&self, host: &str, ip: Option<IpAddr>) -> bool {
        match self {
            Self::Net { ip: net, prefix } => ip.is_some_and(|ip| in_net(ip, *net, *prefix)),
            Self::Host(h) => host == h,
            Self::Suffix(d) => host == d || host.ends_with(&format!(".{d}")),
        }
    }
}

/// IPv4-mapped IPv6 addresses (`::ffff:10.0.0.1`) are their IPv4 address.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

fn in_net(ip: IpAddr, net: IpAddr, prefix: u8) -> bool {
    match (canonical(ip), net) {
        (IpAddr::V4(a), IpAddr::V4(n)) => {
            let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
            u32::from(a) & mask == u32::from(n) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(n)) => {
            let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
            u128::from(a) & mask == u128::from(n) & mask
        }
        _ => false,
    }
}

fn is_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_unspecified()
                || v4.is_link_local()
                // 100.64.0.0/10, carrier-grade NAT
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // unique local
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link local
        }
    }
}

/// `[outbound]` in the config file.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OutboundConfig {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    pub block_private: Option<bool>,
    pub block_metadata: Option<bool>,
}

/// The effective policy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutboundPolicy {
    pub allow: Vec<Rule>,
    pub deny: Vec<Rule>,
    /// Loopback, RFC 1918, carrier-grade NAT, unique-local, link-local and unspecified.
    pub block_private: bool,
    /// Cloud metadata endpoints and link-local addresses.
    pub block_metadata: bool,
}

impl OutboundPolicy {
    /// From the config file; `remote` (the server is reachable from the network) turns
    /// `block_metadata` on unless the file says otherwise.
    pub(crate) fn from_config(c: &OutboundConfig, remote: bool) -> Result<Self, String> {
        let parse = |rules: &[String], what: &str| -> Result<Vec<Rule>, String> {
            rules
                .iter()
                .map(|r| r.parse().map_err(|e| format!("outbound.{what}: {e}")))
                .collect()
        };
        Ok(Self {
            allow: parse(&c.allow, "allow")?,
            deny: parse(&c.deny, "deny")?,
            block_private: c.block_private.unwrap_or(false),
            block_metadata: c.block_metadata.unwrap_or(remote),
        })
    }

    /// Whether the policy restricts anything at all.
    pub fn is_open(&self) -> bool {
        *self == Self::default()
    }
}

impl ConnectPolicy for OutboundPolicy {
    fn check(&self, host: &str, ip: Option<IpAddr>) -> Result<(), String> {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let ip = ip
            .or_else(|| host.trim_matches(['[', ']']).parse().ok())
            .map(canonical);
        if let Some(rule) = self.deny.iter().find(|r| r.matches(&host, ip)) {
            return Err(format!("denied by rule `{rule}`"));
        }
        if self.allow.iter().any(|r| r.matches(&host, ip)) {
            return Ok(());
        }
        if self.block_metadata {
            if METADATA_HOSTS.contains(&host.as_str())
                || ip.is_some_and(|ip| METADATA_IPS.contains(&ip))
            {
                return Err(
                    "cloud metadata endpoints are blocked (outbound.block_metadata)".into(),
                );
            }
            if ip.is_some_and(is_link_local) {
                return Err("link-local addresses are blocked (outbound.block_metadata)".into());
            }
        }
        if self.block_private && ip.is_some_and(is_private) {
            return Err(
                "private and loopback addresses are blocked (outbound.block_private)".into(),
            );
        }
        if !self.allow.is_empty() {
            return Err("not in the outbound allow list".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> Option<IpAddr> {
        Some(s.parse().unwrap())
    }

    fn policy(allow: &[&str], deny: &[&str], private: bool, metadata: bool) -> OutboundPolicy {
        OutboundPolicy::from_config(
            &OutboundConfig {
                allow: allow.iter().map(|s| (*s).to_owned()).collect(),
                deny: deny.iter().map(|s| (*s).to_owned()).collect(),
                block_private: Some(private),
                block_metadata: Some(metadata),
            },
            false,
        )
        .unwrap()
    }

    #[test]
    fn rules() {
        assert_eq!(
            "10.0.0.0/8".parse::<Rule>().unwrap().to_string(),
            "10.0.0.0/8"
        );
        assert_eq!(
            "::ffff:10.1.2.3".parse::<Rule>().unwrap().to_string(),
            "10.1.2.3/32"
        );
        assert_eq!(
            "*.Example.com.".parse::<Rule>().unwrap(),
            Rule::Suffix("example.com".into())
        );
        for bad in ["10.0.0.0/33", "fe80::/129", "x/8", "", "a b", "http://x"] {
            assert!(bad.parse::<Rule>().is_err(), "{bad}");
        }
        let net: Rule = "192.168.0.0/16".parse().unwrap();
        assert!(net.matches("", ip("192.168.4.5")));
        assert!(net.matches("", ip("::ffff:192.168.4.5")), "IPv4-mapped");
        assert!(!net.matches("", ip("192.169.0.1")));
        assert!(
            "0.0.0.0/0"
                .parse::<Rule>()
                .unwrap()
                .matches("", ip("8.8.8.8"))
        );
        let suffix: Rule = "*.corp.test".parse().unwrap();
        assert!(suffix.matches("corp.test", None) && suffix.matches("a.b.corp.test", None));
        assert!(!suffix.matches("evilcorp.test", None));
    }

    #[test]
    fn metadata_and_private() {
        let p = policy(&[], &[], false, true);
        for (host, addr) in [
            ("x", "169.254.169.254"),
            ("x", "fd00:ec2::254"),
            ("x", "::ffff:169.254.169.254"),
            ("x", "fe80::1"),
            ("x", "169.254.3.4"),
        ] {
            assert!(p.check(host, ip(addr)).is_err(), "{addr}");
        }
        assert!(
            p.check("metadata.google.internal", None)
                .unwrap_err()
                .contains("cloud metadata")
        );
        assert!(p.check("x", ip("10.0.0.1")).is_ok());
        assert!(p.check("x", ip("127.0.0.1")).is_ok());

        let p = policy(&[], &[], true, false);
        for addr in [
            "10.1.1.1",
            "172.16.0.1",
            "192.168.1.1",
            "127.0.0.1",
            "::1",
            "0.0.0.0",
            "fc00::1",
            "100.64.0.1",
        ] {
            assert!(
                p.check("x", ip(addr))
                    .unwrap_err()
                    .contains("block_private"),
                "{addr}"
            );
        }
        assert!(p.check("x", ip("93.184.215.14")).is_ok());
        assert!(
            p.check("internal.test", None).is_ok(),
            "names resolved by a proxy are not guessed"
        );
        assert!(
            p.check("10.0.0.1", None).is_err(),
            "IP literal hosts are checked"
        );
    }

    #[test]
    fn allow_and_deny_lists() {
        let p = policy(
            &["*.example.com", "10.20.0.0/16"],
            &["secret.example.com"],
            true,
            true,
        );
        assert!(p.check("api.example.com", ip("93.184.215.14")).is_ok());
        assert!(
            p.check("x", ip("10.20.1.1")).is_ok(),
            "an explicit allow beats block_private"
        );
        assert_eq!(
            p.check("secret.example.com", ip("93.184.215.14"))
                .unwrap_err(),
            "denied by rule `secret.example.com`"
        );
        assert_eq!(
            p.check("other.test", ip("93.184.215.14")).unwrap_err(),
            "not in the outbound allow list"
        );
        assert!(policy(&[], &[], false, false).is_open());
        assert!(
            OutboundPolicy::from_config(&OutboundConfig::default(), true)
                .unwrap()
                .block_metadata
        );
        assert!(
            OutboundPolicy::from_config(
                &OutboundConfig {
                    deny: vec!["10/8".into()],
                    ..OutboundConfig::default()
                },
                false
            )
            .unwrap_err()
            .starts_with("outbound.deny: invalid network")
        );
    }
}

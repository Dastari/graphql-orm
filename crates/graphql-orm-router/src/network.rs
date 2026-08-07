use std::{
    collections::BTreeSet,
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use futures::future::BoxFuture;
use reqwest::{Client, Response, redirect::Policy};
use url::{Host, Url};

use crate::{RouterError, RouterErrorKind};

/// Asynchronous hostname resolution boundary used by dynamic endpoint policy.
pub trait HostResolver: Send + Sync + fmt::Debug + 'static {
    /// Resolves all currently advertised addresses for `host:port`.
    fn resolve(
        &self,
        host: &str,
        port: u16,
    ) -> BoxFuture<'static, Result<Vec<IpAddr>, RouterError>>;
}

/// System DNS resolver used by default.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemHostResolver;

impl HostResolver for SystemHostResolver {
    fn resolve(
        &self,
        host: &str,
        port: u16,
    ) -> BoxFuture<'static, Result<Vec<IpAddr>, RouterError>> {
        let host = host.to_owned();
        Box::pin(async move {
            hive_router::tokio::net::lookup_host((host.as_str(), port))
                .await
                .map(|addresses| addresses.map(|address| address.ip()).collect())
                .map_err(|_| {
                    RouterError::new(
                        RouterErrorKind::NetworkPolicy,
                        "dynamic endpoint hostname resolution failed",
                    )
                })
        })
    }
}

/// One IPv4 or IPv6 network used by the dynamic endpoint allowlist.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NetworkCidr {
    network: IpAddr,
    prefix: u8,
}

impl NetworkCidr {
    /// Returns whether `address` belongs to this network.
    pub fn contains(&self, address: IpAddr) -> bool {
        match (self.network, normalize_ip(address)) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                let mask = ipv4_mask(self.prefix);
                u32::from(network) & mask == u32::from(address) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                let mask = ipv6_mask(self.prefix);
                u128::from(network) & mask == u128::from(address) & mask
            }
            _ => false,
        }
    }
}

impl FromStr for NetworkCidr {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (address, prefix) = value
            .split_once('/')
            .ok_or_else(|| "network must include a prefix length".to_owned())?;
        let address = normalize_ip(
            address
                .parse::<IpAddr>()
                .map_err(|_| "network contains an invalid IP address".to_owned())?,
        );
        let prefix = prefix
            .parse::<u8>()
            .map_err(|_| "network contains an invalid prefix length".to_owned())?;
        let network = match address {
            IpAddr::V4(address) if prefix <= 32 => {
                IpAddr::V4(Ipv4Addr::from(u32::from(address) & ipv4_mask(prefix)))
            }
            IpAddr::V6(address) if prefix <= 128 => {
                IpAddr::V6(Ipv6Addr::from(u128::from(address) & ipv6_mask(prefix)))
            }
            _ => return Err("network prefix length is out of range".to_owned()),
        };
        Ok(Self { network, prefix })
    }
}

impl fmt::Display for NetworkCidr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.network, self.prefix)
    }
}

/// Deny-by-default outbound policy for dynamically advertised endpoints.
#[derive(Clone)]
pub struct NetworkPolicy {
    allowed_hosts: BTreeSet<String>,
    allowed_ports: BTreeSet<u16>,
    allowed_networks: BTreeSet<NetworkCidr>,
    allow_loopback: bool,
    allow_private: bool,
    allow_link_local: bool,
    dns_timeout: Duration,
    max_resolved_addresses: usize,
    resolver: Arc<dyn HostResolver>,
}

impl NetworkPolicy {
    /// Creates a policy that permits HTTP(S) syntax but no destination host or
    /// network until both are explicitly allowlisted.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one exact, case-insensitive hostname or IP-literal allowlist entry.
    #[must_use]
    pub fn allow_host(mut self, host: impl Into<String>) -> Self {
        self.allowed_hosts
            .insert(host.into().trim_end_matches('.').to_ascii_lowercase());
        self
    }

    /// Adds one allowed destination port.
    #[must_use]
    pub fn allow_port(mut self, port: u16) -> Self {
        self.allowed_ports.insert(port);
        self
    }

    /// Adds one post-resolution IPv4 or IPv6 network allowlist entry.
    #[must_use]
    pub fn allow_network(mut self, network: NetworkCidr) -> Self {
        self.allowed_networks.insert(network);
        self
    }

    /// Explicitly permits loopback addresses that are also in an allowed
    /// network. Intended only for test-owned or local-development services.
    #[must_use]
    pub fn allow_loopback(mut self, allowed: bool) -> Self {
        self.allow_loopback = allowed;
        self
    }

    /// Explicitly permits private/unique-local addresses that are also in an
    /// allowed network.
    #[must_use]
    pub fn allow_private(mut self, allowed: bool) -> Self {
        self.allow_private = allowed;
        self
    }

    /// Explicitly permits link-local addresses that are also in an allowed
    /// network. Metadata-service exposure remains the operator's responsibility.
    #[must_use]
    pub fn allow_link_local(mut self, allowed: bool) -> Self {
        self.allow_link_local = allowed;
        self
    }

    /// Sets the bounded DNS resolution timeout.
    #[must_use]
    pub fn with_dns_timeout(mut self, timeout: Duration) -> Self {
        self.dns_timeout = timeout;
        self
    }

    /// Sets the maximum accepted address set for one hostname.
    #[must_use]
    pub fn with_max_resolved_addresses(mut self, maximum: usize) -> Self {
        self.max_resolved_addresses = maximum;
        self
    }

    /// Installs a resolver, primarily for controlled hosts and deterministic
    /// DNS-rebinding tests.
    #[must_use]
    pub fn with_resolver(mut self, resolver: Arc<dyn HostResolver>) -> Self {
        self.resolver = resolver;
        self
    }

    pub(crate) fn validate(&self) -> Result<(), RouterError> {
        if self.allowed_hosts.iter().any(|host| {
            host.is_empty()
                || host.contains('*')
                || host.bytes().any(|byte| byte.is_ascii_whitespace())
        }) || self.allowed_ports.contains(&0)
        {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "dynamic network allowlist contains an invalid host or port",
            ));
        }
        if self.dns_timeout.is_zero() || self.dns_timeout > Duration::from_secs(30) {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "dynamic DNS timeout must be between zero and 30 seconds",
            ));
        }
        if self.max_resolved_addresses == 0 || self.max_resolved_addresses > 64 {
            return Err(RouterError::new(
                RouterErrorKind::InvalidConfiguration,
                "dynamic DNS address bound must be between 1 and 64",
            ));
        }
        Ok(())
    }

    pub(crate) async fn resolve_url(
        &self,
        value: &str,
        field: &str,
    ) -> Result<ResolvedUrl, RouterError> {
        let url = Url::parse(value).map_err(|_| policy_error(field, "is not an absolute URL"))?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(policy_error(
                field,
                "must use HTTP(S) without credentials, query, or fragment",
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| policy_error(field, "does not contain a host"))?
            .trim_end_matches('.')
            .to_ascii_lowercase();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| policy_error(field, "does not contain a supported port"))?;
        if !self.allowed_hosts.contains(&host) || !self.allowed_ports.contains(&port) {
            return Err(policy_error(
                field,
                "destination host or port is not allowlisted",
            ));
        }

        let mut addresses = match url.host() {
            Some(Host::Ipv4(address)) => vec![IpAddr::V4(address)],
            Some(Host::Ipv6(address)) => vec![IpAddr::V6(address)],
            Some(Host::Domain(_)) => hive_router::tokio::time::timeout(
                self.dns_timeout,
                self.resolver.resolve(&host, port),
            )
            .await
            .map_err(|_| policy_error(field, "DNS resolution timed out"))??,
            None => return Err(policy_error(field, "does not contain a host")),
        };
        addresses = addresses.into_iter().map(normalize_ip).collect();
        addresses.sort();
        addresses.dedup();
        if addresses.is_empty() || addresses.len() > self.max_resolved_addresses {
            return Err(policy_error(
                field,
                "resolved address set is empty or exceeds its configured bound",
            ));
        }
        for address in &addresses {
            self.validate_address(*address, field)?;
        }
        Ok(ResolvedUrl {
            url,
            host,
            addresses: addresses
                .into_iter()
                .map(|address| SocketAddr::new(address, port))
                .collect(),
        })
    }

    fn validate_address(&self, address: IpAddr, field: &str) -> Result<(), RouterError> {
        let forbidden = match address {
            IpAddr::V4(address) => {
                address.is_unspecified()
                    || address.is_multicast()
                    || address == Ipv4Addr::BROADCAST
                    || (address.is_loopback() && !self.allow_loopback)
                    || (address.is_private() && !self.allow_private)
                    || (address.is_link_local() && !self.allow_link_local)
            }
            IpAddr::V6(address) => {
                address.is_unspecified()
                    || address.is_multicast()
                    || (address.is_loopback() && !self.allow_loopback)
                    || (address.is_unique_local() && !self.allow_private)
                    || (address.is_unicast_link_local() && !self.allow_link_local)
            }
        };
        if forbidden
            || !self
                .allowed_networks
                .iter()
                .any(|network| network.contains(address))
        {
            return Err(policy_error(
                field,
                "resolved address is forbidden or outside allowed networks",
            ));
        }
        Ok(())
    }

    pub(crate) fn pinned_client(
        &self,
        targets: &[&ResolvedUrl],
        timeout: Duration,
    ) -> Result<Client, RouterError> {
        let mut builder = Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .timeout(timeout);
        for target in targets {
            builder = builder.resolve_to_addrs(&target.host, &target.addresses);
        }
        builder.build().map_err(|_| {
            RouterError::new(
                RouterErrorKind::NetworkPolicy,
                "failed to construct the pinned dynamic endpoint client",
            )
        })
    }

    pub(crate) fn validate_peer(
        &self,
        response: &Response,
        target: &ResolvedUrl,
        field: &str,
    ) -> Result<(), RouterError> {
        let peer = response
            .remote_addr()
            .ok_or_else(|| policy_error(field, "response peer address is unavailable"))?;
        if !target
            .addresses
            .iter()
            .any(|allowed| allowed.ip() == normalize_ip(peer.ip()))
        {
            return Err(policy_error(
                field,
                "response peer did not match the pinned resolution",
            ));
        }
        self.validate_address(peer.ip(), field)
    }
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            allowed_hosts: BTreeSet::new(),
            allowed_ports: [80, 443].into_iter().collect(),
            allowed_networks: BTreeSet::new(),
            allow_loopback: false,
            allow_private: false,
            allow_link_local: false,
            dns_timeout: Duration::from_secs(2),
            max_resolved_addresses: 16,
            resolver: Arc::new(SystemHostResolver),
        }
    }
}

impl fmt::Debug for NetworkPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkPolicy")
            .field("allowed_hosts", &self.allowed_hosts)
            .field("allowed_ports", &self.allowed_ports)
            .field("allowed_networks", &self.allowed_networks)
            .field("allow_loopback", &self.allow_loopback)
            .field("allow_private", &self.allow_private)
            .field("allow_link_local", &self.allow_link_local)
            .field("dns_timeout", &self.dns_timeout)
            .field("max_resolved_addresses", &self.max_resolved_addresses)
            .field("resolver", &"configured")
            .finish()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedUrl {
    pub(crate) url: Url,
    host: String,
    addresses: Vec<SocketAddr>,
}

impl ResolvedUrl {
    pub(crate) fn as_str(&self) -> &str {
        self.url.as_str()
    }
}

fn policy_error(field: &str, detail: &str) -> RouterError {
    RouterError::new(RouterErrorKind::NetworkPolicy, format!("{field} {detail}"))
}

fn normalize_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map_or(IpAddr::V6(address), IpAddr::V4),
        address => address,
    }
}

fn ipv4_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

fn ipv6_mask(prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct FixedResolver(Vec<IpAddr>);

    impl HostResolver for FixedResolver {
        fn resolve(
            &self,
            _host: &str,
            _port: u16,
        ) -> BoxFuture<'static, Result<Vec<IpAddr>, RouterError>> {
            let addresses = self.0.clone();
            Box::pin(async move { Ok(addresses) })
        }
    }

    #[test]
    fn cidr_parsing_canonicalizes_and_contains_addresses() {
        let network = "10.8.1.20/16".parse::<NetworkCidr>().unwrap();
        assert_eq!(network.to_string(), "10.8.0.0/16");
        assert!(network.contains("10.8.9.2".parse().unwrap()));
        assert!(!network.contains("10.9.0.1".parse().unwrap()));
    }

    #[ntex::test]
    async fn resolution_requires_host_port_network_and_special_address_opt_in() {
        let base = NetworkPolicy::new()
            .allow_host("service.internal")
            .allow_port(8080)
            .allow_network("10.0.0.0/8".parse().unwrap())
            .with_resolver(Arc::new(FixedResolver(vec!["10.1.2.3".parse().unwrap()])));
        assert!(
            base.resolve_url("http://service.internal:8080/sdl", "SDL")
                .await
                .is_err()
        );
        let permitted = base.allow_private(true);
        assert!(
            permitted
                .resolve_url("http://service.internal:8080/sdl", "SDL")
                .await
                .is_ok()
        );
    }

    #[ntex::test]
    async fn mixed_safe_and_rebound_dns_answers_fail_closed() {
        let policy = NetworkPolicy::new()
            .allow_host("service.example")
            .allow_network("203.0.113.0/24".parse().unwrap())
            .with_resolver(Arc::new(FixedResolver(vec![
                "203.0.113.10".parse().unwrap(),
                "169.254.169.254".parse().unwrap(),
            ])));
        assert!(
            policy
                .resolve_url("https://service.example/graphql", "GraphQL")
                .await
                .is_err()
        );
    }

    #[ntex::test]
    async fn special_ranges_and_ambiguous_url_components_fail_closed() {
        for (host, network) in [
            ("127.0.0.1", "127.0.0.0/8"),
            ("10.1.2.3", "10.0.0.0/8"),
            ("169.254.10.20", "169.254.0.0/16"),
            ("::1", "::1/128"),
            ("fe80::1", "fe80::/10"),
        ] {
            let policy = NetworkPolicy::new()
                .allow_host(host)
                .allow_network(network.parse().unwrap());
            let url = if host.contains(':') {
                format!("http://[{host}]/graphql")
            } else {
                format!("http://{host}/graphql")
            };
            assert!(policy.resolve_url(&url, "GraphQL").await.is_err());
        }

        let policy = NetworkPolicy::new()
            .allow_host("203.0.113.10")
            .allow_network("203.0.113.0/24".parse().unwrap());
        for url in [
            "http://user@203.0.113.10/graphql",
            "http://203.0.113.10/graphql?target=other",
            "http://203.0.113.10/graphql#fragment",
            "ftp://203.0.113.10/graphql",
        ] {
            assert!(policy.resolve_url(url, "GraphQL").await.is_err());
        }
    }

    #[ntex::test]
    async fn resolution_address_count_is_bounded() {
        let policy = NetworkPolicy::new()
            .allow_host("service.example")
            .allow_network("203.0.113.0/24".parse().unwrap())
            .with_max_resolved_addresses(1)
            .with_resolver(Arc::new(FixedResolver(vec![
                "203.0.113.10".parse().unwrap(),
                "203.0.113.11".parse().unwrap(),
            ])));
        assert!(
            policy
                .resolve_url("https://service.example/graphql", "GraphQL")
                .await
                .is_err()
        );
    }
}

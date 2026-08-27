//! Explicit outbound-network capabilities for untrusted tool execution.

use std::fmt::{Display, Formatter};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkCapability {
    None,
    ModelProviderOnly(NetworkDestination),
    ApprovedDestinations(Vec<NetworkDestination>),
    Unrestricted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkDestination {
    scheme: String,
    host: String,
    port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkDenied {
    reason: String,
}

impl Display for NetworkDenied {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for NetworkDenied {}

impl NetworkCapability {
    #[must_use]
    pub const fn none() -> Self {
        Self::None
    }

    pub fn model_provider(endpoint: &str) -> Result<Self, NetworkDenied> {
        Ok(Self::ModelProviderOnly(NetworkDestination::parse(
            endpoint,
        )?))
    }

    pub fn approved_destinations(destinations: Vec<&str>) -> Result<Self, NetworkDenied> {
        destinations
            .into_iter()
            .map(NetworkDestination::parse)
            .collect::<Result<Vec<_>, _>>()
            .map(Self::ApprovedDestinations)
    }

    #[must_use]
    pub const fn unrestricted() -> Self {
        Self::Unrestricted
    }

    pub fn authorize_url(&self, url: &str) -> Result<(), NetworkDenied> {
        let destination = NetworkDestination::parse(url)?;
        let allowed = match self {
            Self::Unrestricted => true,
            Self::ModelProviderOnly(provider) => provider.matches(&destination),
            Self::ApprovedDestinations(destinations) => {
                destinations.iter().any(|item| item.matches(&destination))
            }
            Self::None => false,
        };
        if allowed {
            Ok(())
        } else {
            Err(NetworkDenied {
                reason: format!("network destination is not authorized: {url}"),
            })
        }
    }

    pub fn authorize_web_url(&self, url: &str) -> Result<(), NetworkDenied> {
        self.resolve_authorized_web_url(url).map(|_| ())
    }

    /// Resolve and authorize a web URL in one operation. Callers that open a
    /// connection must use one of the returned addresses rather than asking
    /// the HTTP client to resolve the hostname again.
    pub fn resolve_authorized_web_url(&self, url: &str) -> Result<Vec<SocketAddr>, NetworkDenied> {
        let parsed = Url::parse(url).map_err(|error| NetworkDenied {
            reason: format!("invalid web URL: {error}"),
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(NetworkDenied {
                reason: format!("unsupported web URL scheme: {}", parsed.scheme()),
            });
        }
        let host = parsed.host_str().ok_or_else(|| NetworkDenied {
            reason: String::from("web URL must contain a host"),
        })?;
        let port = parsed
            .port_or_known_default()
            .ok_or_else(|| NetworkDenied {
                reason: String::from("web URL must use a known or explicit port"),
            })?;
        let addresses = if let Ok(address) = host.parse::<IpAddr>() {
            vec![SocketAddr::new(address, port)]
        } else {
            (host, port)
                .to_socket_addrs()
                .map_err(|error| NetworkDenied {
                    reason: format!("web URL DNS resolution failed: {error}"),
                })?
                .collect::<Vec<_>>()
        };
        if addresses.is_empty() {
            return Err(NetworkDenied {
                reason: String::from("web URL did not resolve to an address"),
            });
        }
        if addresses
            .iter()
            .any(|address| is_private_or_local_address(&address.ip()))
        {
            return Err(NetworkDenied {
                reason: format!("web URL destination is local or private: {host}"),
            });
        }
        self.authorize_url(url)?;
        Ok(addresses)
    }

    pub fn authorize_web_search(&self) -> Result<(), NetworkDenied> {
        if matches!(self, Self::Unrestricted) {
            Ok(())
        } else {
            Err(NetworkDenied {
                reason: String::from("web search requires unrestricted network capability"),
            })
        }
    }
}

fn is_private_or_local_address(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_broadcast()
                || address.is_unspecified()
                || (address.octets()[0] == 100 && (64..=127).contains(&address.octets()[1]))
        }
        IpAddr::V6(address) => {
            address.is_loopback()
                || address.is_unspecified()
                || (address.segments()[0] & 0xfe00) == 0xfc00
                || (address.segments()[0] & 0xffc0) == 0xfe80
                || address
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| is_private_or_local_address(&IpAddr::V4(mapped)))
        }
    }
}

impl NetworkDestination {
    fn parse(value: &str) -> Result<Self, NetworkDenied> {
        let url = Url::parse(value).map_err(|error| NetworkDenied {
            reason: format!("invalid network destination: {error}"),
        })?;
        let Some(host) = url.host_str() else {
            return Err(NetworkDenied {
                reason: String::from("network destination must contain a host"),
            });
        };
        if !matches!(url.scheme(), "http" | "https") {
            return Err(NetworkDenied {
                reason: format!("unsupported network scheme: {}", url.scheme()),
            });
        }
        Ok(Self {
            scheme: url.scheme().to_ascii_lowercase(),
            host: host.to_ascii_lowercase(),
            port: url.port_or_known_default(),
        })
    }

    fn matches(&self, other: &Self) -> bool {
        self.scheme == other.scheme && self.host == other.host && self.port == other.port
    }
}

#[cfg(test)]
mod tests {
    use super::NetworkCapability;

    #[test]
    fn none_denies_web_and_provider_only_denies_arbitrary_hosts() {
        assert!(NetworkCapability::none()
            .authorize_url("https://example.com")
            .is_err());
        let provider = NetworkCapability::model_provider("http://127.0.0.1:8080/v1")
            .expect("provider endpoint should parse");
        assert!(provider.authorize_url("https://example.com").is_err());
        assert!(provider
            .authorize_url("http://127.0.0.1:8080/health")
            .is_ok());
        assert!(provider.authorize_web_search().is_err());
    }

    #[test]
    fn approved_destinations_match_exact_scheme_host_and_port() {
        let capability = NetworkCapability::approved_destinations(vec!["https://example.com"])
            .expect("destination should parse");
        assert!(capability.authorize_url("https://example.com/path").is_ok());
        assert!(capability
            .authorize_url("https://evil-example.com/path")
            .is_err());
        assert!(capability.authorize_url("http://example.com/path").is_err());
        assert!(capability
            .authorize_url("https://example.com:8443/path")
            .is_err());
    }

    #[test]
    fn web_authorization_rejects_local_addresses_and_non_web_schemes() {
        let capability = NetworkCapability::unrestricted();
        assert!(capability
            .authorize_web_url("http://127.0.0.1:8080")
            .is_err());
        assert!(capability
            .authorize_web_url("http://localhost:8080")
            .is_err());
        assert!(capability
            .authorize_web_url("http://169.254.169.254")
            .is_err());
        assert!(capability.authorize_web_url("file:///tmp/secret").is_err());
    }

    #[test]
    fn authorized_web_resolution_returns_only_the_checked_address_set() {
        let capability = NetworkCapability::unrestricted();
        let addresses = capability
            .resolve_authorized_web_url("https://93.184.216.34/")
            .expect("public address should resolve without DNS");
        assert_eq!(addresses.len(), 1);
        assert_eq!(addresses[0].ip().to_string(), "93.184.216.34");
        assert!(capability
            .resolve_authorized_web_url("https://127.0.0.1/")
            .is_err());
    }
}

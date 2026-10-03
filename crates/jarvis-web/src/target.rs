//! What a fetch may be pointed at: the URL rules, and resolution that cannot be redirected after the check.
//!
//! # Validation happens on addresses, after resolution
//!
//! A hostname is not a destination. `Target::resolve` looks the name up **once**, requires **every** answer to be
//! a public address, and returns exactly those addresses for the caller to connect to. The caller pins them
//! (`reqwest::ClientBuilder::resolve_to_addrs`), so the socket goes to what was checked and not to a second lookup
//! an attacker's DNS server could answer differently. Requiring *every* answer closes the mixed-answer trick: a
//! name with one public and one private record is refused rather than being given the chance to choose.
//!
//! An IP literal never reaches DNS, so it is checked as an address directly; a host the `url` crate normalizes
//! (`http://2130706433/`, `http://0x7f.1/`) arrives here already as an `Ipv4` value and is judged as one.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use thiserror::Error;
use url::{Host, Url};

use crate::address::is_public;

/// The longest URL accepted, in characters.
///
/// A URL is the one channel through which a model can send data to a server it chose, so its length is a bound on
/// that channel as well as on memory.
pub const MAX_URL_CHARS: usize = 2048;

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a fetch was refused before anything was sent.
///
/// Every variant is written for the **model** to read, so none names an address or a resolver detail: "this
/// internal host answered" would turn the tool into a way to map a private network.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum Refusal {
    /// The URL is longer than [`MAX_URL_CHARS`].
    #[error("the URL is longer than {MAX_URL_CHARS} characters")]
    TooLong,
    /// The URL could not be parsed.
    #[error("the URL could not be parsed")]
    Malformed,
    /// The scheme is not `http` or `https`.
    #[error("only http and https URLs are fetched")]
    Scheme,
    /// The URL carries a username or password.
    #[error("a URL with a username or password is not fetched")]
    UserInfo,
    /// The URL names no host.
    #[error("the URL names no host")]
    NoHost,
    /// The port is not one this policy allows.
    #[error("that port is not fetched; only the standard http and https ports are")]
    Port,
    /// The host did not resolve.
    #[error("the host could not be resolved")]
    Unresolvable,
    /// The host is, or resolves to, an address outside the public internet.
    #[error("the host is not on the public internet, so it is not fetched")]
    NotPublic,
    /// More redirects than the limit.
    #[error("the page redirected too many times")]
    TooManyRedirects,
    /// A redirect named no location that could be followed.
    #[error("the page redirected to a location that could not be followed")]
    BadRedirect,
}

/// Which destinations a fetch may reach.
#[derive(Clone, Debug)]
pub struct EgressPolicy {
    ports: Vec<u16>,
    /// A loopback port a test server listens on. Present only in test builds: a production policy has no way to
    /// say "allow loopback", so a configuration or a model argument cannot ask for it.
    #[cfg(test)]
    loopback_port: Option<u16>,
}

impl EgressPolicy {
    /// The policy for the public web: ports 80 and 443, public addresses only.
    #[must_use]
    pub fn public_web() -> Self {
        Self {
            ports: vec![80, 443],
            #[cfg(test)]
            loopback_port: None,
        }
    }

    /// A test policy that additionally permits one loopback port, so a local server can stand in for the web.
    #[cfg(test)]
    #[must_use]
    pub fn permitting_loopback_port(port: u16) -> Self {
        Self {
            ports: vec![80, 443, port],
            loopback_port: Some(port),
        }
    }

    /// A test policy that allows one extra port **without** any address exception, so a loopback server on that
    /// port is something the address rule alone must refuse.
    #[cfg(test)]
    #[must_use]
    pub fn permitting_port_only(port: u16) -> Self {
        Self {
            ports: vec![80, 443, port],
            loopback_port: None,
        }
    }

    fn permits_port(&self, port: u16) -> bool {
        self.ports.contains(&port)
    }

    fn permits_address(&self, address: IpAddr, port: u16) -> bool {
        is_public(address) || self.loopback_exception(address, port)
    }

    #[cfg(test)]
    fn loopback_exception(&self, address: IpAddr, port: u16) -> bool {
        address.is_loopback() && self.loopback_port == Some(port)
    }

    /// A production policy has no exception to grant. The method exists so the call site is the same in both
    /// builds, and it takes `self` for the same reason.
    #[cfg(not(test))]
    #[allow(clippy::unused_self)]
    fn loopback_exception(&self, _address: IpAddr, _port: u16) -> bool {
        false
    }
}

/// A URL that passed the static rules.
#[derive(Clone, Debug)]
pub struct Target {
    url: Url,
    port: u16,
}

impl Target {
    /// Applies the static URL rules.
    ///
    /// # Errors
    ///
    /// Returns the [`Refusal`] naming the first rule the URL breaks.
    pub fn parse(text: &str, policy: &EgressPolicy) -> Result<Self, Refusal> {
        if text.chars().count() > MAX_URL_CHARS {
            return Err(Refusal::TooLong);
        }
        let url = Url::parse(text.trim()).map_err(|_| Refusal::Malformed)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Refusal::Scheme);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(Refusal::UserInfo);
        }
        if url.host().is_none() {
            return Err(Refusal::NoHost);
        }
        let port = url.port_or_known_default().ok_or(Refusal::Port)?;
        if !policy.permits_port(port) {
            return Err(Refusal::Port);
        }
        Ok(Self { url, port })
    }

    /// The validated URL.
    #[must_use]
    pub const fn url(&self) -> &Url {
        &self.url
    }

    /// The host name when the URL names a domain, and `None` for an IP literal.
    #[must_use]
    pub fn domain(&self) -> Option<&str> {
        match self.url.host() {
            Some(Host::Domain(domain)) => Some(domain),
            _ => None,
        }
    }

    /// Resolves the host and returns the addresses a connection may use.
    ///
    /// # Errors
    ///
    /// Returns [`Refusal::Unresolvable`] when the lookup fails, times out or is empty, and
    /// [`Refusal::NotPublic`] when **any** answer is not a permitted address.
    pub async fn resolve(&self, policy: &EgressPolicy) -> Result<Vec<SocketAddr>, Refusal> {
        let port = self.port;
        let addresses: Vec<SocketAddr> = match self.url.host() {
            Some(Host::Ipv4(address)) => vec![SocketAddr::new(IpAddr::V4(address), port)],
            Some(Host::Ipv6(address)) => vec![SocketAddr::new(IpAddr::V6(address), port)],
            Some(Host::Domain(domain)) => {
                let lookup = tokio::net::lookup_host((domain, port));
                tokio::time::timeout(RESOLVE_TIMEOUT, lookup)
                    .await
                    .map_err(|_| Refusal::Unresolvable)?
                    .map_err(|_| Refusal::Unresolvable)?
                    .collect()
            }
            None => return Err(Refusal::NoHost),
        };
        check_answers(policy, &addresses)?;
        Ok(addresses)
    }
}

/// Requires a lookup to have answered, and **every** answer to be permitted.
///
/// A name with one public and one private record is refused rather than given the chance to choose: the
/// connection library picks among the answers, and "usually the public one" is not a guard.
fn check_answers(policy: &EgressPolicy, answers: &[SocketAddr]) -> Result<(), Refusal> {
    if answers.is_empty() {
        return Err(Refusal::Unresolvable);
    }
    if answers
        .iter()
        .any(|socket| !policy.permits_address(socket.ip(), socket.port()))
    {
        return Err(Refusal::NotPublic);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Target, Refusal> {
        Target::parse(text, &EgressPolicy::public_web())
    }

    #[test]
    fn the_static_rules_refuse_what_they_name() {
        assert_eq!(parse("not a url").err(), Some(Refusal::Malformed));
        assert_eq!(parse("ftp://example.com/x").err(), Some(Refusal::Scheme));
        assert_eq!(parse("file:///etc/passwd").err(), Some(Refusal::Scheme));
        assert_eq!(
            parse("https://user:pw@example.com/").err(),
            Some(Refusal::UserInfo)
        );
        assert_eq!(
            parse("https://user@example.com/").err(),
            Some(Refusal::UserInfo)
        );
        assert_eq!(
            parse("https://example.com:8080/").err(),
            Some(Refusal::Port)
        );
        assert_eq!(parse("http://example.com:22/").err(), Some(Refusal::Port));
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_CHARS));
        assert_eq!(parse(&long).err(), Some(Refusal::TooLong));
    }

    #[test]
    fn the_standard_ports_and_both_schemes_are_accepted() {
        for text in [
            "https://example.com/a?b=c#d",
            "http://example.com/",
            "https://example.com:443/",
            "http://example.com:80/",
        ] {
            assert!(parse(text).is_ok(), "{text}");
        }
    }

    #[tokio::test]
    async fn an_ip_literal_is_judged_as_an_address_without_a_lookup() {
        let policy = EgressPolicy::public_web();
        for text in [
            "http://127.0.0.1/",
            "http://[::1]/",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.5/",
            "http://[::ffff:127.0.0.1]/",
        ] {
            let target = Target::parse(text, &policy).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(
                target.resolve(&policy).await.err(),
                Some(Refusal::NotPublic),
                "{text}"
            );
        }
    }

    #[tokio::test]
    async fn the_url_parser_normalizes_the_encodings_that_hide_a_loopback_address() {
        // Decimal, hexadecimal and shortened IPv4 spellings are parsed by the `url` crate into the address they
        // mean, so the guard sees `127.0.0.1` and not a string it would have to recognise.
        let policy = EgressPolicy::public_web();
        for text in [
            "http://2130706433/",
            "http://0x7f.0.0.1/",
            "http://127.1/",
            "http://0177.0.0.1/",
        ] {
            let target = Target::parse(text, &policy).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(
                target.resolve(&policy).await.err(),
                Some(Refusal::NotPublic),
                "{text}"
            );
        }
    }

    #[tokio::test]
    async fn a_name_that_resolves_to_loopback_is_refused() {
        let policy = EgressPolicy::public_web();
        let target = Target::parse("http://localhost/", &policy).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            target.resolve(&policy).await.err(),
            Some(Refusal::NotPublic)
        );
    }

    #[test]
    fn one_private_answer_among_public_ones_refuses_the_name() {
        let policy = EgressPolicy::public_web();
        let public = SocketAddr::from(([93, 184, 216, 34], 443));
        let private = SocketAddr::from(([10, 0, 0, 7], 443));
        assert_eq!(check_answers(&policy, &[public]), Ok(()));
        assert_eq!(
            check_answers(&policy, &[public, private]),
            Err(Refusal::NotPublic)
        );
        assert_eq!(
            check_answers(&policy, &[private, public]),
            Err(Refusal::NotPublic)
        );
        assert_eq!(check_answers(&policy, &[]), Err(Refusal::Unresolvable));
    }

    #[tokio::test]
    async fn the_test_exception_names_one_loopback_port_and_nothing_else() {
        let policy = EgressPolicy::permitting_loopback_port(40_001);
        let allowed =
            Target::parse("http://127.0.0.1:40001/", &policy).unwrap_or_else(|e| panic!("{e}"));
        assert!(allowed.resolve(&policy).await.is_ok());
        // A different loopback port is not a port this policy allows at all.
        assert_eq!(
            Target::parse("http://127.0.0.1:40002/", &policy).err(),
            Some(Refusal::Port)
        );
        // And the exception does not widen the address rule for the standard ports.
        let standard =
            Target::parse("http://127.0.0.1/", &policy).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            standard.resolve(&policy).await.err(),
            Some(Refusal::NotPublic)
        );
    }
}

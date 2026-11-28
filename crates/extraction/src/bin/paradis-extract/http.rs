//! Bounded HTTP transport for the configured live sources.

use std::{
    io::{self, Read},
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use reqwest::{
    blocking::{Client, Response},
    dns::{Name, Resolve, Resolving},
    header::{
        ACCEPT, ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap,
        IF_MODIFIED_SINCE, IF_NONE_MATCH, RETRY_AFTER,
    },
    redirect,
};

use super::Disposition;

pub(super) const AGENT: &str = "ParadisExtraction";
pub(super) const REQUEST_INTERVAL: Duration = Duration::from_millis(250);
const USER_AGENT: &str = concat!("ParadisExtraction/", env!("CARGO_PKG_VERSION"));

pub(super) fn client(host: &'static str) -> Result<Client, reqwest::Error> {
    Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(8))
        .dns_resolver(Arc::new(PublicDns { host }))
        .user_agent(USER_AGENT)
        .build()
}

pub(super) struct Reply<R> {
    pub(super) status: u16,
    pub(super) headers: HeaderMap,
    pub(super) body: R,
}

impl<R> Reply<R> {
    pub(super) fn challenge(&self) -> bool {
        self.headers
            .get_all("cf-mitigated")
            .iter()
            .any(|value| value.as_bytes().eq_ignore_ascii_case(b"challenge"))
    }

    pub(super) fn retry_after(&self) -> Option<String> {
        self.headers
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| value.len() <= 64)
            .map(str::to_owned)
    }
}

pub(super) fn get(client: &Client, url: &str, accept: &str) -> Result<Reply<Response>, Failure> {
    get_conditional(client, url, accept, None, None)
}

pub(super) fn get_conditional(
    client: &Client,
    url: &str,
    accept: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<Reply<Response>, Failure> {
    let mut request = client
        .get(url)
        .header(ACCEPT, accept)
        .header(ACCEPT_ENCODING, "identity");
    if let Some(etag) = etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    if let Some(last_modified) = last_modified {
        request = request.header(IF_MODIFIED_SINCE, last_modified);
    }
    let response = request.send().map_err(|error| {
        Failure::skip(if error.is_timeout() {
            "request_timeout"
        } else {
            "request_failed"
        })
    })?;
    Ok(Reply {
        status: response.status().as_u16(),
        headers: response.headers().clone(),
        body: response,
    })
}

#[allow(dead_code)]
pub(super) fn parse_retry_after_seconds(value: &str) -> Option<u64> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .map(|seconds| seconds.min(60))
}

pub(super) fn read_body(
    reply: Reply<impl Read>,
    media_types: &[&str],
    max_bytes: u64,
) -> Result<Vec<u8>, Failure> {
    if reply
        .headers
        .get_all(CONTENT_ENCODING)
        .iter()
        .any(|value| !value.as_bytes().eq_ignore_ascii_case(b"identity"))
    {
        return Err(Failure::skip("unsupported_content_encoding"));
    }
    let content_type = reply
        .headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| Failure::skip("unsupported_content_type"))?;
    let mut parts = content_type.split(';').map(str::trim);
    let Some(media_type) = parts.next().filter(|value| {
        media_types
            .iter()
            .any(|allowed| value.eq_ignore_ascii_case(allowed))
    }) else {
        return Err(Failure::skip("unsupported_content_type"));
    };
    if !media_type.eq_ignore_ascii_case("application/json")
        && parts.any(|parameter| {
            parameter.split_once('=').is_some_and(|(name, value)| {
                name.trim().eq_ignore_ascii_case("charset")
                    && !value.trim().trim_matches('"').eq_ignore_ascii_case("utf-8")
            })
        })
    {
        return Err(Failure::skip("unsupported_charset"));
    }
    let declared_length = reply
        .headers
        .get(CONTENT_LENGTH)
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| Failure::skip("invalid_content_length"))
        })
        .transpose()?;
    if declared_length.is_some_and(|length| length > max_bytes) {
        return Err(Failure::skip("response_too_large"));
    }
    let mut bytes = Vec::new();
    reply
        .body
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure::skip("response_read_failed"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(Failure::skip("response_too_large"));
    }
    if declared_length.is_some_and(|length| length != bytes.len() as u64) {
        return Err(Failure::skip("incomplete_response"));
    }
    Ok(bytes)
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Failure {
    pub(super) disposition: Disposition,
    pub(super) reason: &'static str,
}

impl Failure {
    pub(super) fn skip(reason: &'static str) -> Self {
        Self {
            disposition: Disposition::Skipped,
            reason,
        }
    }

    pub(super) fn restricted(reason: &'static str) -> Self {
        Self {
            disposition: Disposition::Restricted,
            reason,
        }
    }

    pub(super) fn deferred(reason: &'static str) -> Self {
        Self {
            disposition: Disposition::Deferred,
            reason,
        }
    }
}

pub(super) fn check_status(status: u16, challenge: bool) -> Result<(), Failure> {
    if challenge {
        return Err(Failure::restricted("access_challenge"));
    }
    let failure = match status {
        200 => return Ok(()),
        401 | 403 | 451 => Failure::restricted("access_restricted"),
        429 => Failure::deferred("rate_limited"),
        500..=599 => Failure::deferred("service_unavailable"),
        300..=399 => Failure::skip("redirect_or_revalidation"),
        204 | 205 => Failure::skip("no_content"),
        206 => Failure::skip("partial_content"),
        404 | 410 => Failure::skip("missing_resource"),
        _ => Failure::skip("unexpected_http_status"),
    };
    Err(failure)
}

struct PublicDns {
    host: &'static str,
}

impl Resolve for PublicDns {
    fn resolve(&self, name: Name) -> Resolving {
        let host = self.host;
        Box::pin(async move {
            if name.as_str() != host {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "unexpected source host",
                )
                .into());
            }
            // The connector uses these vetted addresses with the original host for TLS.
            let addresses = public_addresses(tokio::net::lookup_host((host, 0)).await?)?;
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

fn public_addresses(answers: impl IntoIterator<Item = SocketAddr>) -> io::Result<Vec<SocketAddr>> {
    let addresses: Vec<_> = answers
        .into_iter()
        .filter(|address| public_ip(address.ip()))
        .collect();
    if addresses.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "no public DNS address",
        ));
    }
    Ok(addresses)
}

fn public_ip(ip: IpAddr) -> bool {
    // Exclude special-use destinations from the IANA IPv4 and IPv6 registries.
    match ip {
        IpAddr::V4(ip) => !matches!(
            ip.octets(),
            [0 | 10 | 127 | 224..=255, ..]
                | [100, 64..=127, ..]
                | [169, 254, ..]
                | [172, 16..=31, ..]
                | [192, 0, 0 | 2, _]
                | [192, 88, 99, _]
                | [192, 168, ..]
                | [198, 18 | 19, ..]
                | [198, 51, 100, _]
                | [203, 0, 113, _]
                | [168, 63, 129, 16]
        ),
        IpAddr::V6(ip) => {
            matches!(ip.segments(), [0x2000..=0x3fff, ..])
                && !matches!(
                    ip.segments(),
                    [0x2001, 0..=0x1ff | 0x0db8, ..] | [0x2002, ..] | [0x3fff, 0..=0x0fff, ..]
                )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{public_addresses, public_ip};
    use std::net::{IpAddr, SocketAddr};

    #[test]
    fn excludes_nonpublic_destinations() {
        for address in [
            "127.0.0.1",
            "100.100.100.200",
            "169.254.169.254",
            "192.0.2.1",
            "168.63.129.16",
            "::1",
            "::ffff:10.0.0.1",
            "2001:db8::1",
            "3fff::1",
            "fd00:ec2::254",
        ] {
            assert!(
                address.parse::<IpAddr>().is_ok_and(|ip| !public_ip(ip)),
                "{address}"
            );
        }
        for address in ["8.8.8.8", "2606:4700::1111"] {
            assert!(address.parse::<IpAddr>().is_ok_and(public_ip), "{address}");
        }
    }

    #[test]
    fn each_resolution_checks_the_current_address_set() {
        let public: SocketAddr = "8.8.8.8:0".parse().unwrap();
        let private: SocketAddr = "127.0.0.1:0".parse().unwrap();
        assert_eq!(public_addresses([private, public]).unwrap(), vec![public]);
        assert!(public_addresses([private]).is_err());
    }
}

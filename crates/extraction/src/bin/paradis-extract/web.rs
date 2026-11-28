//! Source-scoped web acquisition with preflight robots decisions.

use std::{
    collections::HashSet,
    io::{Read, Write},
    thread,
    time::{Duration, Instant},
};

use reqwest::{
    Url,
    header::{AGE, CACHE_CONTROL, HeaderMap, LOCATION},
};
use robotxt::Robots;
use serde::Serialize;
use time::OffsetDateTime;

use super::{
    Disposition, Record, emit,
    http::{self, AGENT, Failure, Reply, check_status, read_body},
};

const ROBOTS_TTL: Duration = Duration::from_secs(600);
const MAX_CRAWL_DELAY: Duration = Duration::from_secs(60);
const MAX_PAGE_BYTES: u64 = 2 * 1024 * 1024;

pub(super) struct Source {
    pub(super) id: &'static str,
    pub(super) host: &'static str,
    pub(super) paths: &'static [&'static str],
    pub(super) robots_paths: &'static [&'static str],
    pub(super) interval: Duration,
    pub(super) content: Content,
}

#[derive(Clone, Copy)]
pub(super) enum Content {
    Page,
    Feed,
}

impl Source {
    fn allows(&self, url: &Url, purpose: Purpose) -> bool {
        let paths = match purpose {
            Purpose::Page | Purpose::Feed => self.paths,
            Purpose::Robots => self.robots_paths,
        };
        url.scheme() == "https"
            && url.host_str() == Some(self.host)
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && paths.contains(&url.path())
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Purpose {
    Robots,
    Page,
    Feed,
}

impl Purpose {
    pub(super) fn accept(self) -> &'static str {
        match self {
            Self::Robots => "text/plain",
            Self::Page => "text/html",
            Self::Feed => "application/rss+xml, application/atom+xml, application/xml, text/xml",
        }
    }

    fn media_types(self) -> &'static [&'static str] {
        match self {
            Self::Robots => &["text/plain"],
            Self::Page => &["text/html"],
            Self::Feed => &[
                "application/rss+xml",
                "application/atom+xml",
                "application/xml",
                "text/xml",
            ],
        }
    }

    fn redirect_limit(self) -> usize {
        match self {
            Self::Robots => 5,
            Self::Page | Self::Feed => 3,
        }
    }
}

#[derive(Debug)]
pub(super) struct Page {
    pub(super) bytes: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub(super) struct RobotsDecision {
    policy_url: String,
    http_status: u16,
    allowed: bool,
    #[serde(with = "time::serde::rfc3339")]
    checked_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
}

struct Rules {
    parsed: Robots,
    url: Url,
    status: u16,
    checked_at: OffsetDateTime,
    expires_at: OffsetDateTime,
}

#[derive(Debug)]
pub(super) struct Attempt {
    purpose: Purpose,
    url: Url,
    parent: Option<Url>,
    time: OffsetDateTime,
    status: Option<u16>,
    retry_after: Option<String>,
    disposition: Disposition,
    reason: Option<&'static str>,
    robots_decision: Option<RobotsDecision>,
}

impl Attempt {
    pub(super) fn fail(&mut self, failure: Failure) {
        self.disposition = failure.disposition;
        self.reason = Some(failure.reason);
    }

    pub(super) fn emit(
        &self,
        output: &mut impl Write,
        source: &Source,
        index: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        emit(
            output,
            Record::RequestAttempt {
                attempt_index: index,
                requested_url: self.url.as_str(),
                final_url: self.status.map(|_| self.url.as_str()),
                discovered_from: self.parent.as_ref().map(Url::as_str),
                attempted_at: self.time,
                access_policy: source.id,
                http_status: self.status,
                retry_after: self.retry_after.as_deref(),
                disposition: self.disposition,
                reason: self.reason,
                document_version_id: None,
                request_kind: Some(self.purpose),
                robots_decision: self.robots_decision.as_ref(),
            },
        )
    }
}

pub(super) fn fetch<R: Read>(
    source: &Source,
    mut url: Url,
    send: impl FnMut(&Url, Purpose) -> Result<Reply<R>, Failure>,
) -> (Vec<Attempt>, Result<Page, Failure>) {
    let mut fetcher = Fetcher {
        source,
        attempts: Vec::new(),
        cache: None,
        last_request: None,
        send,
    };
    url.set_fragment(None);
    let result = fetcher.page(url);
    (fetcher.attempts, result)
}

pub(super) fn acquire<T>(
    source: &Source,
    url: &str,
    output: &mut impl Write,
    prepare: impl FnOnce(&[u8]) -> Result<T, Failure>,
) -> Result<(usize, T), Box<dyn std::error::Error>> {
    let client = http::client(source.host)?;
    let (mut attempts, body) = fetch(source, Url::parse(url)?, |url, purpose| {
        http::get(&client, url.as_str(), purpose.accept())
    });
    let prepared = body.and_then(|body| prepare(&body.bytes));
    if let Err(failure) = prepared
        && let Some(attempt) = attempts.last_mut()
    {
        attempt.fail(failure);
    }
    for (index, attempt) in attempts.iter().enumerate() {
        attempt.emit(output, source, index + 1)?;
    }
    prepared
        .map(|records| (attempts.len(), records))
        .map_err(|failure| format!("{} stopped ({})", source.id, failure.reason).into())
}

struct Fetcher<'a, F> {
    source: &'a Source,
    attempts: Vec<Attempt>,
    cache: Option<Rules>,
    last_request: Option<Instant>,
    send: F,
}

impl<F, R> Fetcher<'_, F>
where
    F: FnMut(&Url, Purpose) -> Result<Reply<R>, Failure>,
    R: Read,
{
    fn page(&mut self, mut url: Url) -> Result<Page, Failure> {
        let purpose = match self.source.content {
            Content::Page => Purpose::Page,
            Content::Feed => Purpose::Feed,
        };
        let mut parent = None;
        let mut seen = HashSet::new();
        loop {
            if !self.source.allows(&url, purpose) {
                let index = self.begin(purpose, &url, parent.as_ref());
                return Err(self.fail(index, Failure::restricted("url_outside_policy")));
            }
            let policy = self.robots_decision(&url);
            let index = self.begin(purpose, &url, parent.as_ref());
            let (decision, delay) = policy.map_err(|failure| {
                let reason = match failure.disposition {
                    Disposition::Deferred => "robots_deferred",
                    Disposition::Restricted => "robots_access_restricted",
                    _ => "robots_unavailable",
                };
                self.fail(index, Failure { reason, ..failure })
            })?;
            let allowed = decision.allowed;
            self.attempts[index].robots_decision = Some(decision);
            if !allowed {
                return Err(self.fail(index, Failure::restricted("robots_disallowed")));
            }
            if delay > MAX_CRAWL_DELAY {
                return Err(self.fail(index, Failure::deferred("robots_crawl_delay")));
            }

            seen.insert(url.clone());
            let reply = self.request(index, delay)?;
            if let Some(next) = self.redirect(index, &reply, &seen)? {
                parent = Some(url);
                url = next;
                continue;
            }
            let bytes = check_status(reply.status, reply.challenge())
                .and_then(|()| read_body(reply, purpose.media_types(), MAX_PAGE_BYTES))
                .map_err(|failure| self.fail(index, failure))?;
            return Ok(Page { bytes });
        }
    }

    fn robots_decision(&mut self, page: &Url) -> Result<(RobotsDecision, Duration), Failure> {
        if self
            .cache
            .as_ref()
            .is_none_or(|rules| rules.expires_at <= OffsetDateTime::now_utc())
        {
            self.cache = Some(self.robots(page)?);
        }
        let cache = self
            .cache
            .as_ref()
            .ok_or_else(|| Failure::skip("robots_unavailable"))?;
        Ok((
            RobotsDecision {
                policy_url: cache.url.as_str().to_owned(),
                http_status: cache.status,
                allowed: cache.parsed.is_url_allowed(page),
                checked_at: cache.checked_at,
                expires_at: cache.expires_at,
            },
            cache.parsed.crawl_delay().unwrap_or_default(),
        ))
    }

    fn robots(&mut self, page: &Url) -> Result<Rules, Failure> {
        let mut url = page
            .join("/robots.txt")
            .map_err(|_| Failure::skip("invalid_robots_url"))?;
        let mut parent = None;
        let mut seen = HashSet::new();
        loop {
            let index = self.begin(Purpose::Robots, &url, parent.as_ref());
            if !self.source.allows(&url, Purpose::Robots) {
                return Err(self.fail(index, Failure::restricted("url_outside_policy")));
            }
            seen.insert(url.clone());
            let reply = self.request(index, Duration::ZERO)?;
            if let Some(next) = self.redirect(index, &reply, &seen)? {
                parent = Some(url);
                url = next;
                continue;
            }
            let status = reply.status;
            let ttl = robots_ttl(&reply.headers);
            let parsed = if !reply.challenge() && matches!(status, 404 | 410) {
                self.fail(index, Failure::skip("robots_not_found"));
                Robots::allow_all()
            } else {
                let bytes = check_status(status, reply.challenge())
                    .and_then(|()| {
                        read_body(
                            reply,
                            Purpose::Robots.media_types(),
                            robotxt::BYTE_LIMIT as u64,
                        )
                    })
                    .map_err(|failure| self.fail(index, failure))?;
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| self.fail(index, Failure::skip("invalid_robots_utf8")))?;
                if text
                    .trim_start_matches('\u{feff}')
                    .trim_start()
                    .starts_with('<')
                {
                    return Err(self.fail(index, Failure::skip("invalid_robots_content")));
                }
                Robots::parse(&bytes, AGENT)
            };
            let checked_at = OffsetDateTime::now_utc();
            return Ok(Rules {
                parsed,
                url,
                status,
                checked_at,
                expires_at: checked_at + ttl,
            });
        }
    }

    fn begin(&mut self, purpose: Purpose, url: &Url, parent: Option<&Url>) -> usize {
        let index = self.attempts.len();
        self.attempts.push(Attempt {
            purpose,
            url: url.clone(),
            parent: parent.cloned(),
            time: OffsetDateTime::now_utc(),
            status: None,
            retry_after: None,
            disposition: Disposition::Accepted,
            reason: None,
            robots_decision: None,
        });
        index
    }

    fn request(&mut self, index: usize, delay: Duration) -> Result<Reply<R>, Failure> {
        if let Some(last) = self.last_request {
            thread::sleep(
                delay
                    .max(self.source.interval)
                    .saturating_sub(last.elapsed()),
            );
        }
        self.last_request = Some(Instant::now());
        let attempt = &mut self.attempts[index];
        attempt.time = OffsetDateTime::now_utc();
        match (self.send)(&attempt.url, attempt.purpose) {
            Ok(reply) => {
                attempt.status = Some(reply.status);
                attempt.retry_after = reply.retry_after();
                Ok(reply)
            }
            Err(failure) => Err(self.fail(index, failure)),
        }
    }

    fn redirect(
        &mut self,
        index: usize,
        reply: &Reply<R>,
        seen: &HashSet<Url>,
    ) -> Result<Option<Url>, Failure> {
        if reply.challenge() || !matches!(reply.status, 301 | 302 | 303 | 307 | 308) {
            return Ok(None);
        }
        let attempt = &self.attempts[index];
        let purpose = attempt.purpose;
        if seen.len() > purpose.redirect_limit() {
            return Err(self.fail(index, Failure::skip("redirect_limit")));
        }
        let mut locations = reply.headers.get_all(LOCATION).iter();
        let Some(mut target) = locations
            .next()
            .filter(|_| locations.next().is_none())
            .and_then(|value| value.to_str().ok())
            .and_then(|value| attempt.url.join(value).ok())
        else {
            return Err(self.fail(index, Failure::skip("invalid_redirect")));
        };
        target.set_fragment(None);
        if !self.source.allows(&target, purpose) {
            return Err(self.fail(index, Failure::restricted("redirect_outside_policy")));
        }
        if seen.contains(&target) {
            return Err(self.fail(index, Failure::skip("redirect_loop")));
        }
        self.attempts[index].disposition = Disposition::Redirected;
        Ok(Some(target))
    }

    fn fail(&mut self, index: usize, failure: Failure) -> Failure {
        self.attempts[index].fail(failure);
        failure
    }
}

fn robots_ttl(headers: &HeaderMap) -> Duration {
    let mut ttl = ROBOTS_TTL;
    for value in headers
        .get_all(CACHE_CONTROL)
        .iter()
        .filter_map(|value| value.to_str().ok())
    {
        for directive in value.split(',').map(str::trim) {
            let (name, value) = directive.split_once('=').unwrap_or((directive, ""));
            let name = name.trim();
            if name.eq_ignore_ascii_case("no-store") || name.eq_ignore_ascii_case("no-cache") {
                return Duration::ZERO;
            }
            if name.eq_ignore_ascii_case("max-age")
                && let Ok(seconds) = value.trim().trim_matches('"').parse::<u64>()
            {
                ttl = ttl.min(Duration::from_secs(seconds));
            }
        }
    }
    let age = headers
        .get(AGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    ttl.saturating_sub(Duration::from_secs(age))
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, io::Cursor};

    use reqwest::header::{
        CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, HeaderValue, RETRY_AFTER,
    };

    use super::*;

    const SOURCE: Source = Source {
        id: "fixture-web-policy",
        host: "approved.example",
        paths: &["/", "/story", "/closed"],
        robots_paths: &["/robots.txt", "/policy.txt"],
        interval: Duration::ZERO,
        content: Content::Page,
    };
    type TestReply = Reply<Cursor<Vec<u8>>>;

    fn reply(status: u16, media_type: &'static str, body: impl Into<Vec<u8>>) -> TestReply {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(media_type));
        Reply {
            status,
            headers,
            body: Cursor::new(body.into()),
        }
    }

    fn redirected(target: &'static str) -> TestReply {
        let mut fixture = reply(302, "text/html", []);
        fixture
            .headers
            .insert(LOCATION, HeaderValue::from_static(target));
        fixture
    }

    fn run(
        path: &str,
        fixtures: impl IntoIterator<Item = (&'static str, Result<TestReply, Failure>)>,
    ) -> (Vec<Attempt>, Result<Page, Failure>) {
        let mut fixtures: VecDeque<_> = fixtures.into_iter().collect();
        let url = Url::parse(&format!("https://approved.example{path}")).unwrap();
        let result = fetch(&SOURCE, url, |url, _| {
            let Some((expected_path, response)) = fixtures.pop_front() else {
                panic!("unexpected request");
            };
            assert_eq!(url.host_str(), Some(SOURCE.host));
            assert_eq!(url.path(), expected_path);
            assert_eq!(url.query(), None);
            assert_eq!(url.fragment(), None);
            response
        });
        assert!(fixtures.is_empty(), "some expected requests did not occur");
        result
    }

    #[test]
    fn records_robots_decisions_and_rechecks_each_page_redirect() {
        let (attempts, result) = run("/", [
            ("/robots.txt", Ok(redirected("/policy.txt"))),
            ("/policy.txt", Ok(reply(200, "text/plain", b"User-agent: *\nDisallow: /\n\nUser-agent: PARADISEXTRACTION\nDisallow: /closed\n\nUser-agent: ParadisExtraction\nAllow: /story\nDisallow: /story".as_slice()))),
            ("/", Ok(redirected("/story#fragment"))),
            ("/story", Ok(redirected("/closed"))),
        ]);
        assert_eq!(result.unwrap_err().reason, "robots_disallowed");
        assert_eq!(attempts.len(), 5);
        assert_eq!(attempts[0].disposition, Disposition::Redirected);
        assert_eq!(attempts[1].disposition, Disposition::Accepted);
        assert_eq!(attempts[2].disposition, Disposition::Redirected);
        assert_eq!(attempts[3].disposition, Disposition::Redirected);
        assert_eq!(attempts[4].status, None);
        let denied = attempts[4].robots_decision.as_ref().unwrap();
        assert!(!denied.allowed);
        assert_eq!(denied.policy_url, "https://approved.example/policy.txt");
        assert!(denied.expires_at > denied.checked_at);
    }

    #[test]
    fn fails_closed_on_robots_errors_challenges_throttling_and_bad_redirects() {
        for (mut fixture, reason, disposition) in [
            (
                reply(403, "text/plain", []),
                "robots_access_restricted",
                Disposition::Restricted,
            ),
            (
                reply(429, "text/plain", []),
                "robots_deferred",
                Disposition::Deferred,
            ),
            (
                reply(503, "text/plain", []),
                "robots_deferred",
                Disposition::Deferred,
            ),
            (
                reply(200, "text/html", []),
                "robots_unavailable",
                Disposition::Skipped,
            ),
            (
                reply(200, "text/plain", b"<html>Access blocked</html>".as_slice()),
                "robots_unavailable",
                Disposition::Skipped,
            ),
            (
                redirected("https://other.example/robots.txt"),
                "robots_access_restricted",
                Disposition::Restricted,
            ),
        ] {
            fixture
                .headers
                .insert(RETRY_AFTER, HeaderValue::from_static("30"));
            let (attempts, result) = run("/", [("/robots.txt", Ok(fixture))]);
            assert_eq!(result.unwrap_err().reason, reason);
            assert_eq!(attempts[0].retry_after.as_deref(), Some("30"));
            assert_eq!(attempts[1].status, None);
            assert_eq!(attempts[1].disposition, disposition);
        }
        let mut challenge = reply(404, "text/plain", []);
        challenge
            .headers
            .insert("cf-mitigated", HeaderValue::from_static("challenge"));
        let (attempts, result) = run("/", [("/robots.txt", Ok(challenge))]);
        assert_eq!(result.unwrap_err().disposition, Disposition::Restricted);
        assert_eq!(attempts[0].reason, Some("access_challenge"));
        let (attempts, result) = run(
            "/",
            [("/robots.txt", Err(Failure::skip("request_timeout")))],
        );
        assert_eq!(result.unwrap_err().reason, "robots_unavailable");
        assert_eq!(attempts[0].reason, Some("request_timeout"));
        assert_eq!(attempts[1].status, None);
    }

    #[test]
    fn uses_fresh_rules_after_an_uncacheable_policy() {
        let mut missing = reply(404, "text/html", []);
        missing
            .headers
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        let (attempts, result) = run(
            "/",
            [
                ("/robots.txt", Ok(missing)),
                ("/", Ok(redirected("/story"))),
                (
                    "/robots.txt",
                    Ok(reply(
                        200,
                        "text/plain",
                        b"User-agent: *\nDisallow: /story".as_slice(),
                    )),
                ),
            ],
        );
        assert_eq!(result.unwrap_err().reason, "robots_disallowed");
        assert_eq!(attempts[0].reason, Some("robots_not_found"));
        assert!(attempts[1].robots_decision.as_ref().unwrap().allowed);
        assert!(!attempts[3].robots_decision.as_ref().unwrap().allowed);
    }

    #[test]
    fn rejects_unsafe_locations_before_another_request() {
        for location in [
            "https://other.example/story",
            "http://approved.example/story",
            "https://approved.example:8443/story",
            "https://user@approved.example/story",
            "/unknown",
            "/story?unapproved=1",
        ] {
            let (attempts, result) = run(
                "/",
                [
                    ("/robots.txt", Ok(reply(410, "text/plain", []))),
                    ("/", Ok(redirected(location))),
                ],
            );
            assert_eq!(result.unwrap_err().reason, "redirect_outside_policy");
            assert_eq!(attempts.len(), 2);
        }
        let (attempts, result) = run("/unknown", []);
        assert_eq!(result.unwrap_err().reason, "url_outside_policy");
        assert_eq!(attempts.len(), 1);
    }

    #[test]
    fn page_response_checks_stop_before_parsing_restricted_or_incomplete_content() {
        let mut compressed = reply(200, "text/html", []);
        compressed
            .headers
            .insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        let mut incomplete = reply(200, "text/html", b"hello".as_slice());
        incomplete
            .headers
            .insert(CONTENT_LENGTH, HeaderValue::from_static("6"));
        let mut challenge = reply(302, "text/html", []);
        challenge
            .headers
            .insert("cf-mitigated", HeaderValue::from_static("challenge"));
        for (fixture, reason) in [
            (compressed, "unsupported_content_encoding"),
            (incomplete, "incomplete_response"),
            (challenge, "access_challenge"),
            (
                reply(200, "text/html; charset=iso-8859-1", []),
                "unsupported_charset",
            ),
            (reply(206, "text/html", []), "partial_content"),
            (
                reply(200, "text/html", vec![b'x'; MAX_PAGE_BYTES as usize + 1]),
                "response_too_large",
            ),
        ] {
            let (_, result) = run(
                "/",
                [
                    ("/robots.txt", Ok(reply(404, "text/html", []))),
                    ("/", Ok(fixture)),
                ],
            );
            assert_eq!(result.unwrap_err().reason, reason);
        }
        let (_, result) = run(
            "/",
            [
                ("/robots.txt", Ok(reply(200, "text/plain", []))),
                (
                    "/",
                    Ok(reply(
                        200,
                        "text/html; charset=UTF-8",
                        b"<main>article</main>".as_slice(),
                    )),
                ),
            ],
        );
        assert_eq!(result.unwrap().bytes, b"<main>article</main>");
    }

    #[test]
    fn feed_requests_require_robots_permission_and_xml_media() {
        let source = Source {
            paths: &["/rss"],
            content: Content::Feed,
            ..SOURCE
        };
        let url = Url::parse("https://approved.example/rss").unwrap();
        let mut requests = Vec::new();
        let (_, blocked) = fetch(&source, url.clone(), |url, purpose| {
            requests.push(url.path().to_owned());
            assert!(matches!(purpose, Purpose::Robots));
            Ok(reply(
                200,
                "text/plain",
                b"User-agent: *\nDisallow: /rss".as_slice(),
            ))
        });
        assert_eq!(requests, ["/robots.txt"]);
        assert_eq!(blocked.unwrap_err().reason, "robots_disallowed");

        let mut requests = Vec::new();
        let (_, result) = fetch(&source, url, |url, purpose| {
            requests.push(url.path().to_owned());
            Ok(match purpose {
                Purpose::Robots => reply(404, "text/plain", []),
                Purpose::Feed => reply(
                    200,
                    "application/rss+xml; charset=UTF-8",
                    b"<rss/>".as_slice(),
                ),
                Purpose::Page => panic!("feed route requested HTML"),
            })
        });
        assert_eq!(requests, ["/robots.txt", "/rss"]);
        assert_eq!(result.unwrap().bytes, b"<rss/>");
    }
}

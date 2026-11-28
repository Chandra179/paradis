//! Conservative page screening for inert HTML snapshots.

use html2text::{Comment, Document, Element, Handle};
use serde::Serialize;

use crate::normalize::excluded;

const SCREENING_VERSION: &str = "html-screen-v1";
// Positive signatures are short notices. Longer content is left to the normalizer.
const MAX_FINGERPRINT_BYTES: usize = 1024;
const MAX_FINGERPRINT_FRAGMENTS: usize = 128;

/// The outcome and inspectable signals from screening one saved HTML page.
#[derive(Debug, Serialize)]
pub struct PageScreening {
    version: &'static str,
    signals: Vec<&'static str>,
    #[serde(skip)]
    decision: PageDecision,
}

impl PageScreening {
    fn new(decision: PageDecision, signals: &[&'static str]) -> Self {
        Self {
            version: SCREENING_VERSION,
            signals: signals.to_vec(),
            decision,
        }
    }

    pub(crate) fn blank(zero_bytes: bool) -> Self {
        let reason = if zero_bytes {
            "empty_input"
        } else {
            "blank_input"
        };
        Self::new(PageDecision::Skipped(reason), &[reason])
    }

    /// Returns the page-level decision with a stable reason for withheld pages.
    pub fn decision(&self) -> PageDecision {
        self.decision
    }

    pub(crate) fn record_fallback(&mut self, signal: &'static str) {
        self.signals.push("empty_primary_region");
        self.signals.push(signal);
    }
}

/// Whether this HTML snapshot can proceed to text normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageDecision {
    /// The screening rules did not withhold the page.
    Eligible,
    /// The page was withheld for a recognized reason.
    Skipped(&'static str),
    /// The snapshot did not establish whether primary content is available.
    Uncertain(&'static str),
}

pub(crate) fn inspect(
    root: &Handle,
    mut render_leaf: impl FnMut(&Handle) -> Option<String>,
) -> PageScreening {
    let page = Fingerprints::from_html(root, &mut render_leaf);
    let body = page.body.as_str();
    let modal = page.modal.as_str();
    let noscript = page.noscript.as_str();
    let javascript_notice = noscript.is_some_and(|text| {
        matches!(
            text.trim_end_matches('.'),
            "please enable javascript to continue"
                | "you need to enable javascript to run this app"
                | "this site requires javascript"
        )
    });

    if let Some(text) = body {
        if page.heading_matches(&["404", "404 not found", "page not found"])
            && page.notice_matches(
                text,
                &[
                    "we couldn't find the page you requested",
                    "we could not find the page you requested",
                    "the requested page could not be found",
                    "the page you are looking for does not exist",
                ],
            )
        {
            return PageScreening::new(
                PageDecision::Skipped("likely_soft_404"),
                &["not_found_heading", "not_found_notice"],
            );
        }
        if page.heading_matches(&["domain for sale", "this domain is for sale"])
            && page.notice_matches(
                text,
                &[
                    "this domain is for sale",
                    "buy this domain",
                    "this domain is parked",
                ],
            )
        {
            return PageScreening::new(
                PageDecision::Skipped("likely_parked_domain"),
                &["domain_sale_heading", "domain_sale_notice"],
            );
        }
    }

    if body == Some("")
        && modal.is_some_and(|text| {
            (text.contains("cookie preferences")
                || text.contains("cookie settings")
                || text.contains("we use cookies"))
                && text.contains("accept all")
                && (text.contains("reject all") || text.contains("decline all"))
        })
    {
        return PageScreening::new(
            PageDecision::Skipped("consent_only"),
            &["no_text_outside_modal", "cookie_consent_controls"],
        );
    }

    let blank_or_loading = body.is_some_and(|text| {
        matches!(
            text.trim_end_matches(['.', '…']),
            "" | "loading" | "loading application"
        )
    });
    if blank_or_loading
        && modal == Some("")
        && page.script
        && (noscript == Some("") || javascript_notice)
        && (page.mount || javascript_notice)
    {
        let mut signals = vec!["no_primary_text", "script_present"];
        if page.mount {
            signals.push("application_mount");
        }
        if javascript_notice {
            signals.push("javascript_required_notice");
        }
        return PageScreening::new(PageDecision::Uncertain("likely_javascript_shell"), &signals);
    }

    if body == Some("") && modal == Some("") && matches!(noscript, Some("")) {
        return PageScreening::new(
            PageDecision::Skipped("no_meaningful_html_body"),
            &["no_visible_body_text"],
        );
    }

    PageScreening::new(PageDecision::Eligible, &[])
}

#[derive(Debug, Default)]
struct Fingerprint {
    value: String,
    overflowed: bool,
    fragments: usize,
}

impl Fingerprint {
    fn append(&mut self, text: &str) {
        if self.overflowed {
            return;
        }
        for word in text.split_whitespace() {
            let separator = usize::from(!self.value.is_empty());
            if self.value.len() + separator + word.len() > MAX_FINGERPRINT_BYTES {
                self.overflowed = true;
                break;
            }
            if separator != 0 {
                self.value.push(' ');
            }
            self.value
                .extend(word.chars().map(|character| match character {
                    '’' | '‘' => '\'',
                    _ => character.to_ascii_lowercase(),
                }));
        }
    }

    fn allow_fragment(&mut self) -> bool {
        self.fragments += 1;
        self.overflowed |= self.fragments > MAX_FINGERPRINT_FRAGMENTS;
        !self.overflowed
    }

    fn as_str(&self) -> Option<&str> {
        (!self.overflowed).then_some(self.value.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Outside,
    Title,
    Body,
    Heading,
    Modal,
    Noscript,
}

#[derive(Debug, Default)]
struct Fingerprints {
    title: Fingerprint,
    body: Fingerprint,
    heading: Fingerprint,
    modal: Fingerprint,
    noscript: Fingerprint,
    script: bool,
    mount: bool,
}

impl Fingerprints {
    fn from_html(root: &Handle, render_leaf: &mut impl FnMut(&Handle) -> Option<String>) -> Self {
        let mut page = Self::default();
        let mut pending = vec![(root.clone(), Zone::Outside)];
        while let Some((node, mut zone)) = pending.pop() {
            match &node.data {
                Element { name, attrs, .. } => {
                    let tag = name.local.as_ref();
                    let attrs = attrs.borrow();
                    let attr = |key| {
                        attrs
                            .iter()
                            .find(|attribute| &*attribute.name.local == key)
                            .map(|attribute| attribute.value.as_ref())
                    };
                    if attr("hidden").is_some()
                        || attr("aria-hidden")
                            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
                    {
                        continue;
                    }
                    match tag {
                        "script" => {
                            page.script |= matches!(
                                attr("type"),
                                None | Some(
                                    "" | "module" | "text/javascript" | "application/javascript"
                                )
                            );
                            continue;
                        }
                        "title" if zone == Zone::Outside => zone = Zone::Title,
                        "body" => zone = Zone::Body,
                        "h1" if zone == Zone::Body => zone = Zone::Heading,
                        "noscript" if zone == Zone::Body => zone = Zone::Noscript,
                        "head" => {}
                        "dialog" if attr("open").is_some() && zone == Zone::Body => {
                            zone = Zone::Modal
                        }
                        "dialog" => continue,
                        _ if zone == Zone::Body
                            && matches!(attr("role"), Some("dialog" | "alertdialog"))
                            && attr("aria-modal") == Some("true") =>
                        {
                            zone = Zone::Modal;
                        }
                        _ if excluded(&node) => continue,
                        "img" => page.append(zone, attr("alt").unwrap_or_default()),
                        _ => {}
                    }
                    page.mount |= zone == Zone::Body
                        && matches!(attr("id"), Some("app" | "root" | "__next" | "__nuxt"));
                }
                Document | Comment { .. } => {}
                _ if page.allow_fragment(zone) => {
                    if let Some(text) = render_leaf(&node) {
                        page.append(zone, &text);
                    }
                }
                _ => {}
            }
            pending.extend(
                node.children
                    .borrow()
                    .iter()
                    .rev()
                    .map(|child| (child.clone(), zone)),
            );
        }
        page
    }

    fn allow_fragment(&mut self, zone: Zone) -> bool {
        match zone {
            Zone::Outside => false,
            Zone::Title => self.title.allow_fragment(),
            Zone::Body => self.body.allow_fragment(),
            Zone::Heading => {
                let body = self.body.allow_fragment();
                let heading = self.heading.allow_fragment();
                body || heading
            }
            Zone::Modal => self.modal.allow_fragment(),
            Zone::Noscript => self.noscript.allow_fragment(),
        }
    }

    fn append(&mut self, zone: Zone, text: &str) {
        match zone {
            Zone::Outside => {}
            Zone::Title => self.title.append(text),
            Zone::Body => self.body.append(text),
            Zone::Heading => {
                self.body.append(text);
                self.heading.append(text);
            }
            Zone::Modal => self.modal.append(text),
            Zone::Noscript => self.noscript.append(text),
        }
    }

    fn heading_matches(&self, headings: &[&str]) -> bool {
        [self.title.as_str(), self.heading.as_str()]
            .into_iter()
            .flatten()
            .any(|heading| headings.contains(&heading.trim_end_matches(['.', ':'])))
    }

    fn notice_matches(&self, body: &str, notices: &[&str]) -> bool {
        let message = self
            .heading
            .as_str()
            .filter(|heading| !heading.is_empty())
            .and_then(|heading| body.strip_prefix(heading))
            .and_then(|rest| rest.strip_prefix(' '))
            .unwrap_or(body);
        notices.contains(&message.trim_end_matches('.'))
    }
}

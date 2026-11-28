//! Text representations for plain text and inert HTML snapshots.

use std::rc::Rc;

use html2text::{Element, Handle, RcDom, config};
use serde::Serialize;

use crate::{
    DocumentMetadata, DocumentVersion, EvidenceError, PageDecision, PageScreening, metadata,
    screening,
};

/// The supported UTF-8 snapshot formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputFormat {
    /// Plain text with normalized line endings.
    Text,
    /// Main text selected from a parsed HTML document.
    Html,
    /// Rendered HTML body before main-content selection and pruning.
    HtmlBody,
}

/// Page screening signals and any usable version of the supplied snapshot.
#[derive(Debug)]
pub struct SnapshotInspection {
    /// The normalized version, when the input reached extraction with text.
    pub document: Option<DocumentVersion>,
    /// The recorded screening outcome for saved HTML in the normal pipeline.
    pub page_screening: Option<PageScreening>,
    /// Local publisher metadata extracted from HTML `<meta>` and JSON-LD.
    pub metadata: Option<DocumentMetadata>,
}

impl InputFormat {
    /// Screens a saved snapshot before constructing its normalized document.
    ///
    /// # Errors
    ///
    /// Returns an error if normalization fails or the source ID is blank.
    pub fn inspect(
        self,
        source_id: &str,
        snapshot: &str,
    ) -> Result<SnapshotInspection, EvidenceError> {
        let (text, page_screening, version, metadata) = if self == Self::Html {
            let input = snapshot.trim_start_matches('\u{feff}');
            if input.trim().is_empty() {
                (
                    String::new(),
                    Some(PageScreening::blank(snapshot.is_empty())),
                    self.version(),
                    None,
                )
            } else {
                let parsed = normalize_html(input, true, true)?;
                let version = parsed.fallback_version.unwrap_or(self.version());
                (parsed.text, parsed.page_screening, version, parsed.metadata)
            }
        } else {
            let metadata = if self == Self::HtmlBody {
                metadata::extract_from_html(snapshot.trim_start_matches('\u{feff}'))
            } else {
                None
            };
            (
                self.normalize_text(snapshot)?,
                None,
                self.version(),
                metadata,
            )
        };
        let document = if text.is_empty() {
            None
        } else {
            Some(DocumentVersion::from_normalized(
                source_id,
                snapshot.as_bytes(),
                version,
                text,
            )?)
        };
        Ok(SnapshotInspection {
            document,
            page_screening,
            metadata,
        })
    }

    /// Builds a version from a UTF-8 input using the selected format.
    ///
    /// # Errors
    ///
    /// Returns an error if normalization fails or the source ID is blank.
    pub fn normalize(
        self,
        source_id: &str,
        snapshot: &str,
    ) -> Result<DocumentVersion, EvidenceError> {
        DocumentVersion::from_normalized(
            source_id,
            snapshot.as_bytes(),
            self.version(),
            self.normalize_text(snapshot)?,
        )
    }

    /// Normalizes text from a source field without constructing a document version.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTML cannot be parsed.
    pub fn normalize_text(self, input: &str) -> Result<String, EvidenceError> {
        let input = input.trim_start_matches('\u{feff}');
        match self {
            Self::Text => Ok(input
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .trim()
                .to_owned()),
            Self::Html | Self::HtmlBody => {
                normalize_html(input, self == Self::Html, false).map(|parsed| parsed.text)
            }
        }
    }

    fn version(self) -> &'static str {
        match self {
            Self::Text => "plain-text-v1",
            Self::Html => "semantic-html-v1",
            Self::HtmlBody => "body-html-v1",
        }
    }
}

#[derive(Debug, Default)]
struct NormalizedHtml {
    text: String,
    page_screening: Option<PageScreening>,
    fallback_version: Option<&'static str>,
    metadata: Option<DocumentMetadata>,
}

fn normalize_html(
    snapshot: &str,
    select_main: bool,
    screen: bool,
) -> Result<NormalizedHtml, EvidenceError> {
    const WRAP_COLUMNS: usize = 4096;
    const MAX_ELEMENTS: usize = 100_000;
    const MAX_SELECTED_BYTES: usize = 2 * 1024 * 1024;
    let renderer = config::plain_no_decorate().no_table_borders();
    let document = renderer
        .parse_html(snapshot.as_bytes())
        .map_err(|_| EvidenceError::InvalidHtml)?;

    let mut page_screening = screen.then(|| {
        let mut fragment = RcDom::default();
        screening::inspect(&document.document, |leaf| {
            fragment.document = leaf.clone();
            renderer
                .dom_to_render_tree(&fragment)
                .and_then(|tree| renderer.render_to_string(tree, WRAP_COLUMNS))
                .ok()
        })
    });
    if page_screening
        .as_ref()
        .is_some_and(|page| page.decision() != PageDecision::Eligible)
    {
        return Ok(NormalizedHtml {
            page_screening,
            ..NormalizedHtml::default()
        });
    }

    let render = |document: &RcDom| {
        renderer
            .dom_to_render_tree(document)
            .and_then(|tree| renderer.render_to_string(tree, WRAP_COLUMNS))
            .map(|text| text.trim().to_owned())
            .map_err(|_| EvidenceError::InvalidHtml)
    };
    let metadata = metadata::extract_from_html(snapshot);

    if select_main {
        if !has_semantic_landmark(&document.document)
            && let Some(text) = try_legible(snapshot, &renderer, MAX_ELEMENTS, MAX_SELECTED_BYTES)
        {
            return Ok(NormalizedHtml {
                text,
                page_screening,
                fallback_version: Some("legible-html-v1"),
                metadata,
            });
        }
        prune_content(find_content(&document.document, false));
    }

    let text = render(&document)?;
    if text.is_empty()
        && let Some(screening) = &mut page_screening
    {
        // Selection prunes siblings. Reparse only on this recovery path.
        drop(document);
        let fallback = renderer
            .parse_html(snapshot.as_bytes())
            .map_err(|_| EvidenceError::InvalidHtml)?;
        for (region, signal) in marked_regions(&fallback.document) {
            prune_descendants(&region);
            let fragment = RcDom {
                document: region,
                ..RcDom::default()
            };
            if let Ok(text) = render(&fragment)
                && !text.is_empty()
            {
                screening.record_fallback(signal);
                return Ok(NormalizedHtml {
                    text,
                    page_screening,
                    fallback_version: Some("semantic-html-marked-region-v1"),
                    metadata,
                });
            }
        }
        prune_content(find_content(&fallback.document, true));
        let text = render(&fallback)?;
        let body_fallback = !text.is_empty();
        if body_fallback {
            screening.record_fallback("body_fallback_used");
        }
        return Ok(NormalizedHtml {
            text,
            page_screening,
            fallback_version: body_fallback.then_some("semantic-html-body-fallback-v1"),
            metadata,
        });
    }
    Ok(NormalizedHtml {
        text,
        page_screening,
        fallback_version: None,
        metadata,
    })
}

fn prune_content(content: Handle) {
    let mut node = content.clone();
    // Narrow the document to the selected region while preserving its ancestor structure.
    while let Some(parent) = node.get_parent() {
        parent
            .children
            .borrow_mut()
            .retain(|child| Rc::ptr_eq(child, &node));
        node = parent;
    }
    prune_descendants(&content);
}

fn prune_descendants(content: &Handle) {
    let mut pending = vec![content.clone()];
    while let Some(node) = pending.pop() {
        let mut children = node.children.borrow_mut();
        children.retain(|child| !excluded(child));
        pending.extend(children.iter().cloned());
    }
}

fn has_semantic_landmark(root: &Handle) -> bool {
    visible_descendants(root).any(|node| {
        matches!(
            &node.data,
            Element { name, .. } if matches!(name.local.as_ref(), "main" | "article")
        )
    })
}

fn try_legible<D: html2text::render::TextDecorator>(
    snapshot: &str,
    renderer: &config::Config<D>,
    max_elements: usize,
    max_selected_bytes: usize,
) -> Option<String> {
    let options = legible::Options::new().max_elems_to_parse(max_elements);
    let article = legible::parse(snapshot, None, Some(options)).ok()?;
    if article.content.len() > max_selected_bytes {
        return None;
    }
    let parsed = renderer.parse_html(article.content.as_bytes()).ok()?;
    let tree = renderer.dom_to_render_tree(&parsed).ok()?;
    let text = renderer.render_to_string(tree, 4096).ok()?;
    let trimmed = text.trim().to_owned();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn find_content(root: &Handle, prefer_body: bool) -> Handle {
    let mut body = root.clone();
    let mut article = None;
    for node in visible_descendants(root) {
        if let Element { name, .. } = &node.data {
            match name.local.as_ref() {
                "main" if !prefer_body => return node,
                "article" if article.is_none() => article = Some(node.clone()),
                "body" if prefer_body => return node,
                "body" => body = node.clone(),
                _ => {}
            }
        }
    }
    article.unwrap_or(body)
}

fn marked_regions(root: &Handle) -> impl Iterator<Item = (Handle, &'static str)> {
    let mut article = None;
    let mut main = None;
    for node in visible_descendants(root) {
        let Element { attrs, .. } = &node.data else {
            continue;
        };
        let attrs = attrs.borrow();
        if article.is_none()
            && attrs.iter().any(|attr| {
                &*attr.name.local == "itemprop"
                    && attr
                        .value
                        .split_ascii_whitespace()
                        .any(|property| property == "articleBody")
            })
        {
            article = Some((node.clone(), "article_body_marker_used"));
        }
        if main.is_none()
            && attrs.iter().any(|attr| {
                &*attr.name.local == "role"
                    && attr.value.split_ascii_whitespace().next() == Some("main")
            })
        {
            main = Some((node.clone(), "main_role_marker_used"));
        }
        if article.is_some() && main.is_some() {
            break;
        }
    }
    article.into_iter().chain(main)
}

fn visible_descendants(root: &Handle) -> impl Iterator<Item = Handle> {
    let mut pending = vec![root.clone()];
    std::iter::from_fn(move || {
        while let Some(node) = pending.pop() {
            if !excluded(&node) {
                pending.extend(node.children.borrow().iter().rev().cloned());
                return Some(node);
            }
        }
        None
    })
}

pub(crate) fn excluded(node: &Handle) -> bool {
    let Element { name, attrs, .. } = &node.data else {
        return false;
    };
    matches!(
        name.local.as_ref(),
        "head" | "nav" | "aside" | "footer" | "script" | "style" | "template" | "noscript"
    ) || attrs
        .borrow()
        .iter()
        .any(|attr| match attr.name.local.as_ref() {
            "hidden" => true,
            "aria-hidden" => attr.value.as_ref() == "true",
            "role" => matches!(
                attr.value.as_ref(),
                "navigation" | "complementary" | "contentinfo"
            ),
            _ => false,
        })
}

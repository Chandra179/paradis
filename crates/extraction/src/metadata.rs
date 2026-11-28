//! Local HTML metadata and Schema.org JSON-LD extraction without network resolution.

use html2text::{Element, Handle, RcDom, config};
use serde::Serialize;
use serde_json::Value;

const MAX_JSON_LD_BYTES: usize = 256 * 1024;
const MAX_JSON_LD_SCRIPTS: usize = 16;

/// Publisher metadata extracted locally from HTML `<meta>` tags and Schema.org JSON-LD.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct DocumentMetadata {
    /// Headline or title from JSON-LD or HTML metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Origin of the selected title (`json_ld`, `og_title`, or `html_title`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_origin: Option<&'static str>,
    /// Publication timestamp/date string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    /// Origin of the publication timestamp (`json_ld` or `html_meta`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_origin: Option<&'static str>,
    /// Modification timestamp/date string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
    /// Origin of the modification timestamp (`json_ld` or `html_meta`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_origin: Option<&'static str>,
    /// Declared author names.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// Origin of the author list (`json_ld` or `html_meta`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_origin: Option<&'static str>,
    /// Recorded disagreements between metadata sources (e.g. JSON-LD vs. HTML meta).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disagreements: Vec<MetadataDisagreement>,
}

/// A preserved discrepancy between two local metadata origins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetadataDisagreement {
    /// Field name (`title`, `published_at`, or `modified_at`).
    pub field: &'static str,
    /// Origin of the primary value.
    pub primary_origin: &'static str,
    /// Primary value.
    pub primary_value: String,
    /// Origin of the disagreeing value.
    pub secondary_origin: &'static str,
    /// Secondary value.
    pub secondary_value: String,
}

impl DocumentMetadata {
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.published_at.is_none()
            && self.modified_at.is_none()
            && self.authors.is_empty()
    }
}

/// Extracts `<title>`, `<meta>`, and local `<script type="application/ld+json">` fields.
pub(crate) fn extract_from_html(html: &str) -> Option<DocumentMetadata> {
    let renderer = config::plain_no_decorate().no_table_borders();
    let dom = renderer.parse_html(html.as_bytes()).ok()?;

    let mut html_title: Option<String> = None;
    let mut og_title: Option<String> = None;
    let mut meta_published: Option<String> = None;
    let mut meta_modified: Option<String> = None;
    let mut meta_authors: Vec<String> = Vec::new();
    let mut json_ld_scripts = 0usize;
    let mut ld_title: Option<String> = None;
    let mut ld_published: Option<String> = None;
    let mut ld_modified: Option<String> = None;
    let mut ld_authors: Vec<String> = Vec::new();

    let mut pending = vec![dom.document.clone()];
    while let Some(node) = pending.pop() {
        if let Element { name, .. } = &node.data {
            let local: &str = &name.local;
            if local.eq_ignore_ascii_case("title") && html_title.is_none() {
                let text = render_text(&renderer, &node);
                if !text.is_empty() {
                    html_title = Some(text);
                }
            } else if local.eq_ignore_ascii_case("meta") {
                let key = attr_value(&node, "property")
                    .or_else(|| attr_value(&node, "name"))
                    .map(|s| s.to_ascii_lowercase());
                let content = attr_value(&node, "content").map(|s| s.trim().to_owned());
                if let (Some(key), Some(content)) = (key, content)
                    && !content.is_empty()
                {
                    match key.as_str() {
                        "og:title" if og_title.is_none() => og_title = Some(content),
                        "article:published_time" | "datepublished" | "pubdate"
                            if meta_published.is_none() =>
                        {
                            meta_published = Some(content);
                        }
                        "article:modified_time" | "datemodified" | "lastmod"
                            if meta_modified.is_none() =>
                        {
                            meta_modified = Some(content);
                        }
                        "author" | "article:author" if !meta_authors.contains(&content) => {
                            meta_authors.push(content);
                        }
                        _ => {}
                    }
                }
            } else if local.eq_ignore_ascii_case("script") && json_ld_scripts < MAX_JSON_LD_SCRIPTS
            {
                let is_json_ld = attr_value(&node, "type")
                    .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/ld+json"));
                if is_json_ld {
                    json_ld_scripts += 1;
                    let raw = extract_script_body(&renderer, &node);
                    if !raw.is_empty()
                        && raw.len() <= MAX_JSON_LD_BYTES
                        && let Ok(value) = serde_json::from_str::<Value>(&raw)
                    {
                        inspect_json_ld(
                            &value,
                            &mut ld_title,
                            &mut ld_published,
                            &mut ld_modified,
                            &mut ld_authors,
                        );
                    }
                }
            }
        }
        pending.extend(node.children.borrow().iter().rev().cloned());
    }

    let mut meta = DocumentMetadata::default();
    if let Some(title) = ld_title {
        meta.title = Some(title);
        meta.title_origin = Some("json_ld");
        if let Some(other) = og_title.or(html_title)
            && meta.title.as_deref() != Some(other.as_str())
        {
            meta.disagreements.push(MetadataDisagreement {
                field: "title",
                primary_origin: "json_ld",
                primary_value: meta.title.clone().unwrap_or_default(),
                secondary_origin: "html_meta",
                secondary_value: other,
            });
        }
    } else if let Some(title) = og_title {
        meta.title = Some(title);
        meta.title_origin = Some("og_title");
    } else if let Some(title) = html_title {
        meta.title = Some(title);
        meta.title_origin = Some("html_title");
    }

    assign_field(
        "published_at",
        (ld_published, "json_ld"),
        (meta_published, "html_meta"),
        &mut meta.published_at,
        &mut meta.published_origin,
        &mut meta.disagreements,
    );
    assign_field(
        "modified_at",
        (ld_modified, "json_ld"),
        (meta_modified, "html_meta"),
        &mut meta.modified_at,
        &mut meta.modified_origin,
        &mut meta.disagreements,
    );

    if !ld_authors.is_empty() {
        meta.authors = ld_authors;
        meta.author_origin = Some("json_ld");
    } else if !meta_authors.is_empty() {
        meta.authors = meta_authors;
        meta.author_origin = Some("html_meta");
    }

    (!meta.is_empty()).then_some(meta)
}

fn assign_field(
    field: &'static str,
    primary: (Option<String>, &'static str),
    secondary: (Option<String>, &'static str),
    out_value: &mut Option<String>,
    out_origin: &mut Option<&'static str>,
    disagreements: &mut Vec<MetadataDisagreement>,
) {
    match (primary.0, secondary.0) {
        (Some(p), Some(s)) => {
            if p != s {
                disagreements.push(MetadataDisagreement {
                    field,
                    primary_origin: primary.1,
                    primary_value: p.clone(),
                    secondary_origin: secondary.1,
                    secondary_value: s,
                });
            }
            *out_value = Some(p);
            *out_origin = Some(primary.1);
        }
        (Some(p), None) => {
            *out_value = Some(p);
            *out_origin = Some(primary.1);
        }
        (None, Some(s)) => {
            *out_value = Some(s);
            *out_origin = Some(secondary.1);
        }
        (None, None) => {}
    }
}

fn inspect_json_ld(
    value: &Value,
    title: &mut Option<String>,
    published: &mut Option<String>,
    modified: &mut Option<String>,
    authors: &mut Vec<String>,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                inspect_json_ld(item, title, published, modified, authors);
            }
        }
        Value::Object(map) => {
            if let Some(graph) = map.get("@graph") {
                inspect_json_ld(graph, title, published, modified, authors);
            }
            if is_article_type(map.get("@type")) {
                if title.is_none() {
                    *title = non_empty_str(map.get("headline"))
                        .or_else(|| non_empty_str(map.get("name")));
                }
                if published.is_none() {
                    *published = non_empty_str(map.get("datePublished"));
                }
                if modified.is_none() {
                    *modified = non_empty_str(map.get("dateModified"));
                }
                if authors.is_empty()
                    && let Some(author) = map.get("author")
                {
                    collect_authors(author, authors);
                }
            }
        }
        _ => {}
    }
}

fn is_article_type(ty: Option<&Value>) -> bool {
    const TYPES: [&str; 5] = [
        "Article",
        "NewsArticle",
        "BlogPosting",
        "Report",
        "ScholarlyArticle",
    ];
    match ty {
        Some(Value::String(s)) => TYPES.iter().any(|t| s.eq_ignore_ascii_case(t)),
        Some(Value::Array(items)) => items.iter().any(|item| is_article_type(Some(item))),
        _ => false,
    }
}

fn collect_authors(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            let trimmed = s.trim();
            if !trimmed.is_empty() && !out.iter().any(|existing| existing == trimmed) {
                out.push(trimmed.to_owned());
            }
        }
        Value::Object(map) => {
            if let Some(name) = non_empty_str(map.get("name"))
                && !out.iter().any(|existing| existing == &name)
            {
                out.push(name);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_authors(item, out);
            }
        }
        _ => {}
    }
}

fn non_empty_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn attr_value(node: &Handle, name: &str) -> Option<String> {
    let Element { attrs, .. } = &node.data else {
        return None;
    };
    attrs
        .borrow()
        .iter()
        .find(|attr| (*attr.name.local).eq_ignore_ascii_case(name))
        .map(|attr| attr.value.as_ref().to_owned())
}

fn render_text<D: html2text::render::TextDecorator>(
    renderer: &config::Config<D>,
    root: &Handle,
) -> String {
    let mut out = String::new();
    for child in root.children.borrow().iter() {
        let fragment = RcDom {
            document: child.clone(),
            ..RcDom::default()
        };
        if let Ok(tree) = renderer.dom_to_render_tree(&fragment)
            && let Ok(rendered) = renderer.render_to_string(tree, 4096)
        {
            out.push_str(&rendered);
        }
    }
    out.trim().to_owned()
}

fn extract_script_body<D: html2text::render::TextDecorator>(
    renderer: &config::Config<D>,
    script_node: &Handle,
) -> String {
    // Wrap children in a temporary <div> so html2text renders text nodes inside <script>.
    let mut raw = String::new();
    for child in script_node.children.borrow().iter() {
        let fragment = RcDom {
            document: child.clone(),
            ..RcDom::default()
        };
        if let Ok(tree) = renderer.dom_to_render_tree(&fragment)
            && let Ok(rendered) = renderer.render_to_string(tree, 65_536)
        {
            raw.push_str(&rendered);
        }
    }
    raw.trim().to_owned()
}

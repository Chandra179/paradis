//! Limits the approved Hacker News page to normalized story title fields.

use std::{collections::HashSet, error::Error, io::Write, iter::from_fn};

use extraction::{DocumentVersion, Extractor};
use html2text::{Element, Handle, RcDom, config, render::TrivialDecorator};

use super::{
    DocumentHistory, HackerNewsRecord, SourceRecord, emit_document,
    http::{self, Failure},
    web::{self, Source},
};

const URL: &str = "https://news.ycombinator.com/";
const SOURCE: Source = Source {
    id: "hacker-news-front-page-v1-title-only",
    host: "news.ycombinator.com",
    paths: &["/"],
    robots_paths: &["/robots.txt"],
    interval: http::REQUEST_INTERVAL,
    content: web::Content::Page,
};

pub(super) fn extract(
    limit: usize,
    extractor: &Extractor,
    output: &mut impl Write,
) -> Result<(), Box<dyn Error>> {
    let (attempt_index, documents) =
        web::acquire(&SOURCE, URL, output, |bytes| parse_page(bytes, limit))?;
    let mut history = DocumentHistory::default();
    for (document, source_record) in documents {
        emit_document(
            output,
            attempt_index,
            &document,
            Some(SourceRecord::HackerNews(source_record)),
            &extractor.extract(&document)?,
            history.observe(attempt_index, &document),
        )?;
    }
    Ok(())
}

fn parse_page(
    bytes: &[u8],
    limit: usize,
) -> Result<Vec<(DocumentVersion, HackerNewsRecord)>, Failure> {
    std::str::from_utf8(bytes).map_err(|_| Failure::skip("invalid_utf8"))?;
    let renderer = config::with_decorator(TrivialDecorator::new()).no_table_borders();
    let page = renderer
        .parse_html(bytes)
        .map_err(|_| Failure::skip("normalization_failed"))?;
    let scored_stories: HashSet<u64> = elements(&page.document)
        .filter_map(|node| {
            element(&node, "span", "score")
                .then(|| attribute(&node, "id"))??
                .strip_prefix("score_")?
                .parse()
                .ok()
        })
        .collect();

    let mut seen = HashSet::new();
    let mut documents = Vec::new();
    let mut fragment = RcDom::default();
    for row in elements(&page.document).filter(|node| element(node, "tr", "athing")) {
        let Some(id) = attribute(&row, "id").and_then(|id| id.parse::<u64>().ok()) else {
            continue;
        };
        // Jobs and other rows without a story score are outside the title pilot.
        if !scored_stories.contains(&id) || !seen.insert(id) {
            continue;
        }
        let Some(title) = elements(&row)
            .find(|node| element(node, "span", "titleline"))
            .and_then(|node| {
                node.children
                    .borrow()
                    .iter()
                    .find(|child| element(child, "a", ""))
                    .cloned()
            })
        else {
            continue;
        };
        fragment.document = title;
        let text = renderer
            .dom_to_render_tree(&fragment)
            .and_then(|tree| renderer.render_to_string(tree, 4096))
            .map_err(|_| Failure::skip("normalization_failed"))?;
        let text = text.trim().to_owned();
        if text.is_empty() {
            continue;
        }
        let document = DocumentVersion::from_normalized(
            &format!("{URL}#story-{id}"),
            bytes,
            "hn-front-page-title-v1",
            text,
        )
        .map_err(|_| Failure::skip("invalid_document_version"))?;
        documents.push((
            document,
            HackerNewsRecord {
                id,
                locator: format!("tr[id=\"{id}\"] .titleline > a").into(),
                id_locator: None,
                published_at: None,
            },
        ));
        if documents.len() == limit {
            break;
        }
    }
    if documents.is_empty() {
        return Err(Failure::skip("no_story_titles_found"));
    }
    Ok(documents)
}

fn elements(root: &Handle) -> impl Iterator<Item = Handle> {
    let mut pending = vec![root.clone()];
    from_fn(move || {
        let node = pending.pop()?;
        pending.extend(node.children.borrow().iter().rev().cloned());
        Some(node)
    })
}

fn element(node: &Handle, tag: &str, class: &str) -> bool {
    matches!(&node.data, Element { name, .. } if &*name.local == tag)
        && (class.is_empty()
            || attribute(node, "class")
                .is_some_and(|classes| classes.split_ascii_whitespace().any(|name| name == class)))
}

fn attribute(node: &Handle, key: &str) -> Option<String> {
    let Element { attrs, .. } = &node.data else {
        return None;
    };
    attrs
        .borrow()
        .iter()
        .find(|attribute| &*attribute.name.local == key)
        .map(|attribute| attribute.value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_only_scored_story_titles_and_their_page_locators() {
        let bytes = br#"
          <table>
            <tr class="athing submission" id="12"><td><span class="titleline">
              <a href="https://example.test/external">The caf&eacute; uses 20 kWh</a><span>external-site</span>
            </span></td></tr>
            <tr><td><span class="score" id="score_12">18 points</span> hidden-user</td></tr>
            <tr class="athing submission" id="13"><td><span class="titleline">
              <a href="https://jobs.example.test/">Unscored job entry</a>
            </span></td></tr>
          </table>
          <script src="https://script.example.test/private"></script>
        "#;
        let documents = parse_page(bytes, 25).unwrap();
        assert_eq!(documents.len(), 1);
        let (document, source) = &documents[0];
        assert_eq!(source.id, 12);
        assert_eq!(source.locator, "tr[id=\"12\"] .titleline > a");
        assert_eq!(document.text(), "The café uses 20 kWh");
        assert_eq!(
            document.source_id(),
            "https://news.ycombinator.com/#story-12"
        );
        let json = serde_json::to_string(&(document, source)).unwrap();
        for irrelevant in [
            "example.test",
            "external-site",
            "18 points",
            "hidden-user",
            "Unscored job",
        ] {
            assert!(!json.contains(irrelevant), "{irrelevant}");
        }
    }
}

//! Runs Rust Readability ports on one local, bounded HTML snapshot.

use std::{
    env,
    error::Error,
    fs::{self, File},
    io::Read,
    path::Path,
};

use serde_json::json;

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ELEMENTS: usize = 100_000;
type SelectArticle = fn(&str) -> Result<String, &'static str>;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let method = args.next().ok_or("expected a comparator name")?;
    let file = args.next().ok_or("expected a saved HTML snapshot")?;
    if args.next().is_some() {
        return Err("expected one saved HTML snapshot".into());
    }
    let (version, selector): (&str, SelectArticle) = match method.to_str() {
        Some("dom_smoothie") => ("0.20.0", dom_smoothie),
        Some("legible") => ("0.5.1", legible),
        _ => return Err("unknown Readability comparator".into()),
    };
    let result = read_snapshot(Path::new(&file)).and_then(|text| selector(&text));
    let (content, error) = match result {
        Ok(content) if content.len() as u64 <= MAX_BYTES => (content, None),
        Ok(_) => (String::new(), Some("selected_html_too_large")),
        Err(reason) => (String::new(), Some(reason)),
    };
    println!(
        "{}",
        json!({"content": content, "packages": {"version": version}, "error": error})
    );
    Ok(())
}

fn read_snapshot(path: &Path) -> Result<String, &'static str> {
    if !fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        return Err("unreadable_input");
    }
    let file = File::open(path).map_err(|_| "unreadable_input")?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "unreadable_input")?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("input_too_large");
    }
    String::from_utf8(bytes).map_err(|_| "invalid_utf8")
}

fn dom_smoothie(html: &str) -> Result<String, &'static str> {
    let options = dom_smoothie::Config {
        max_elements_to_parse: MAX_ELEMENTS,
        ..dom_smoothie::Config::default()
    };
    let mut selector =
        dom_smoothie::Readability::new(html, Some("https://benchmark.invalid/"), Some(options))
            .map_err(|_| "parse_failed")?;
    selector
        .parse()
        .map(|article| article.content.to_string())
        .map_err(|error| match error {
            dom_smoothie::ReadabilityError::TooManyElements(..) => "too_many_elements",
            dom_smoothie::ReadabilityError::GrabFailed => "main_content_not_found",
            _ => "parse_failed",
        })
}

fn legible(html: &str) -> Result<String, &'static str> {
    let options = legible::Options::new().max_elems_to_parse(MAX_ELEMENTS);
    legible::parse(html, Some("https://benchmark.invalid/"), Some(options))
        .map(|article| article.content)
        .map_err(|error| match error {
            legible::Error::TooManyElements(..) => "too_many_elements",
            legible::Error::NoContent | legible::Error::NoBody => "main_content_not_found",
            _ => "parse_failed",
        })
}

//! Deterministic baseline for explicit dates, quantities, and relations.

use std::ops::Range;

use regex::Regex;
use serde::Serialize;
use time::{Date, Month, macros::format_description};

use crate::{DependencyError, DependencyParse, DocumentVersion, EvidenceError, EvidenceSpan};

/// Maximum emitted candidates per document.
pub(crate) const MAX_CANDIDATES: usize = 2_000;
const NUMBER: &str = r"[+\-−]?(?:[0-9]{1,3}(?:,[0-9]{3})+|[0-9]+)(?:\.[0-9]+)?";
const SCALE: &str = "thousand|million|billion|trillion";
const MONTH: &str =
    "January|February|March|April|May|June|July|August|September|October|November|December";
const EXTRACTOR_VERSION: &str = "surface-v4";
const SYNTAX_VERSION: &str = "dependency-v1";

/// Field-level UTF-8 byte spans for a relationship candidate's arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArgumentEvidence {
    /// Exact span of the subject in the normalized document.
    pub subject: EvidenceSpan,
    /// Exact span of the predicate in the normalized document.
    pub predicate: EvidenceSpan,
    /// Exact span of the object in the normalized document.
    pub object: EvidenceSpan,
}

/// A proposed annotation with its supporting text and method.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    /// Version of the extraction method.
    pub extractor_version: &'static str,
    /// Identifier for the rule that found this candidate.
    pub rule_id: &'static str,
    /// Parser and model used to generate dependency annotations, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub syntax_parser: Option<String>,
    /// Parsed value or relation arguments.
    pub value: CandidateValue,
    /// Exact matching text in the normalized document.
    pub evidence: EvidenceSpan,
    /// Exact spans for relationship subject, predicate, and object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_evidence: Option<ArgumentEvidence>,
}

/// The interpretation proposed for a text span.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CandidateValue {
    /// An unambiguous calendar date.
    Date {
        /// Full date in ISO 8601 form.
        iso_date: String,
    },
    /// A value with a recognizable unit.
    Quantity {
        /// Decimal representation that preserves precision.
        value: String,
        /// Unit in the source wording.
        unit: String,
    },
    /// An explicitly phrased relation for review.
    Relationship {
        /// Surface text of the subject.
        subject: String,
        /// Surface text of the predicate.
        predicate: String,
        /// Surface text of the object.
        object: String,
    },
}

/// The baseline extraction result for one document.
#[derive(Debug)]
pub struct Extraction {
    /// Candidates in document order.
    pub candidates: Vec<Candidate>,
    /// Additional candidates were omitted due to the configured output bound.
    pub truncated: bool,
}

/// Compiled patterns shared across a batch of inputs.
#[derive(Debug)]
pub struct Extractor {
    date: Regex,
    quantity: Regex,
    relationship: Regex,
    event_relationship: Regex,
    coordinator: Regex,
    simple_noun_tail: Regex,
    clause_boundary: Regex,
    uncertain: Regex,
}

impl Extractor {
    /// Compiles the versioned general-purpose baseline patterns.
    ///
    /// # Errors
    ///
    /// Returns an error if a built-in pattern fails to compile.
    pub fn new() -> Result<Self, regex::Error> {
        Ok(Self {
            date: Regex::new(&format!(
                r"(?i)\b(?:(?P<iso>[0-9]{{4}}-[0-9]{{2}}-[0-9]{{2}})|(?:(?P<d1>[0-9]{{1,2}})(?P<s1>st|nd|rd|th)?[ \t]+(?P<m1>{MONTH})[ \t]+|(?P<m2>{MONTH})[ \t]+(?P<d2>[0-9]{{1,2}})(?P<s2>st|nd|rd|th)?,?[ \t]+)(?P<year>[0-9]{{4}}))\b",
            ))?,
            quantity: Regex::new(&format!(
                r"(?ix)(?:(?P<prefix>[€£$])[\t\x20]*(?P<currency>{NUMBER})\b(?:[\t\x20]+(?P<currency_scale>{SCALE})\b)?|
                (?P<number>{NUMBER})(?:[\t\x20]+(?P<unit_scale>{SCALE})\b)?[\t\x20]*(?P<unit>%|(?:percentage[\t\x20]+points?|percent|milliseconds?|seconds?|minutes?|hours?|days?|weeks?|months?|years?|kilograms?|grams?|kilomet(?:er|re)s?|met(?:er|re)s?|lit(?:er|re)s?|bytes?|kg|mg|km|cm|mm|kWh|USD|EUR|GBP)\b))",
            ))?,
            relationship: Regex::new(
                r"(?i)^(?P<subject>[\p{L}][\p{L}\p{M}\p{N}'’&/()\- ]{0,100}?) +(?P<predicate>is located in|is part of|belongs to|consists of|contains|includes|uses|produces) +(?P<object>[\p{L}\p{N}].{0,160})$",
            )?,
            event_relationship: Regex::new(
                r"(?i)^(?P<subject>[\p{L}][\p{L}\p{M}\p{N}'’&/()\- ]{0,100}?) +(?P<predicate>(?:was|were) +(?:founded|acquired|built|released|published|developed|created|launched|written|designed|established) +(?:by|in|on|for)|is +(?:based|headquartered) +in|announced|released|launched|acquired|founded|published|developed|created|weighs|supports|decreases|increases|reduces|made) +(?P<object>[\p{L}\p{N}].{0,160})$",
            )?,
            coordinator: Regex::new(r"(?i)\b(?:and|or|nor|but)\b")?,
            simple_noun_tail: Regex::new(r"(?i)^(?:and|or) +(?:a|an|the) +[\p{L}\p{M}'’\-]+$")?,
            clause_boundary: Regex::new(r"(?m)[.!?]+(?:[\t ]+|$)|\n+")?,
            uncertain: Regex::new(
                r#"(?i)\b(?:if|unless|not|never|may|might|could|would|should|reportedly|says?|said|according\s+to)\b|[":;“”]"#,
            )?,
        })
    }

    /// Finds bounded, evidence-linked candidates in document order.
    ///
    /// # Errors
    ///
    /// Returns an error if a match fails evidence validation.
    pub fn extract(&self, document: &DocumentVersion) -> Result<Extraction, EvidenceError> {
        self.extract_with_relationships(document, self.relationships(document.text()), None)
    }

    /// Compares dependency-based relations while retaining the surface date and quantity rules.
    ///
    /// # Errors
    ///
    /// Returns an error if the supplied parse does not align to the document or its evidence.
    pub fn extract_with_dependencies(
        &self,
        document: &DocumentVersion,
        dependencies: &DependencyParse,
    ) -> Result<Extraction, DependencyError> {
        let relationships = dependencies.relationships(document, &self.uncertain)?;
        self.extract_with_relationships(
            document,
            relationships.into_iter(),
            Some(dependencies.parser()),
        )
        .map_err(DependencyError::Evidence)
    }

    fn extract_with_relationships(
        &self,
        document: &DocumentVersion,
        relationships: impl Iterator<Item = Match>,
        syntax_parser: Option<&str>,
    ) -> Result<Extraction, EvidenceError> {
        let text = document.text();
        let mut matches = self
            .dates(text)
            .take(MAX_CANDIDATES + 1)
            .chain(self.quantities(text).take(MAX_CANDIDATES + 1))
            .map(|item| (item, None))
            .chain(
                relationships
                    .take(MAX_CANDIDATES + 1)
                    .map(|item| (item, syntax_parser)),
            )
            .collect::<Vec<_>>();
        matches.sort_by_key(|(item, _)| (item.range.start, item.range.end));
        let truncated = matches.len() > MAX_CANDIDATES;

        let candidates = matches
            .into_iter()
            .take(MAX_CANDIDATES)
            .map(|(item, parser)| {
                let argument_evidence = match item.argument_ranges {
                    Some([subject, predicate, object]) => Some(ArgumentEvidence {
                        subject: document.evidence(subject)?,
                        predicate: document.evidence(predicate)?,
                        object: document.evidence(object)?,
                    }),
                    None => None,
                };
                Ok(Candidate {
                    extractor_version: if parser.is_some() {
                        SYNTAX_VERSION
                    } else {
                        EXTRACTOR_VERSION
                    },
                    rule_id: item.rule_id,
                    syntax_parser: parser.map(str::to_owned),
                    value: item.value,
                    evidence: document.evidence(item.range)?,
                    argument_evidence,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Extraction {
            candidates,
            truncated,
        })
    }

    fn dates<'a>(&'a self, text: &'a str) -> impl Iterator<Item = Match> + 'a {
        self.date.captures_iter(text).filter_map(|captures| {
            let mention = captures.get(0)?;
            if captures.name("iso").is_some() {
                let date = Date::parse(
                    mention.as_str(),
                    format_description!("[year]-[month]-[day]"),
                )
                .ok()?;
                return calendar_date(date, mention.range(), "iso_date");
            }
            let mut month = captures
                .name("m1")
                .or_else(|| captures.name("m2"))?
                .as_str()
                .to_ascii_lowercase();
            month.get_mut(..1)?.make_ascii_uppercase();
            let month: Month = month.parse().ok()?;
            let day = captures
                .name("d1")
                .or_else(|| captures.name("d2"))?
                .as_str()
                .parse()
                .ok()?;
            let suffix = captures.name("s1").or_else(|| captures.name("s2"));
            if suffix
                .is_some_and(|suffix| !suffix.as_str().eq_ignore_ascii_case(ordinal_suffix(day)))
            {
                return None;
            }
            let year = captures.name("year")?.as_str().parse().ok()?;
            calendar_date(
                Date::from_calendar_date(year, month, day).ok()?,
                mention.range(),
                if suffix.is_some() {
                    "named_ordinal_date"
                } else {
                    "named_date"
                },
            )
        })
    }

    fn quantities<'a>(&'a self, text: &'a str) -> impl Iterator<Item = Match> + 'a {
        self.quantity.captures_iter(text).filter_map(|captures| {
            let span = captures.get(0)?;
            let number = captures
                .name("number")
                .or_else(|| captures.name("currency"))?;
            let unit = captures.name("unit").or_else(|| captures.name("prefix"))?;
            let prefix = &text[..span.start()];
            let tail = &text[span.end()..];
            let before = prefix.chars().next_back();
            let after = tail.chars().next();
            if before.is_some_and(|ch| {
                ch.is_alphanumeric()
                    || matches!(
                        ch,
                        '.' | ',' | '_' | '+' | '-' | '−' | '–' | '—' | '$' | '€' | '£'
                    )
            }) || after.is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '/' | '^'))
                || prefix
                    .trim_end_matches([' ', '\t'])
                    .ends_with(['/', '⁄', '∕'])
                || matches!(tail.as_bytes(), [b'.' | b',', b'0'..=b'9', ..])
            {
                return None;
            }
            let scale = captures
                .name("currency_scale")
                .or_else(|| captures.name("unit_scale"));
            let value = number.as_str().replace(',', "").replace('−', "-");
            let value = match scale {
                Some(scale) => scale_decimal(&value, scale.as_str())?,
                None => value,
            };
            Some(Match {
                range: span.range(),
                rule_id: if scale.is_some() {
                    "scaled_number_with_unit"
                } else {
                    "number_with_unit"
                },
                value: CandidateValue::Quantity {
                    value,
                    unit: unit.as_str().to_owned(),
                },
                argument_ranges: None,
            })
        })
    }

    fn relationships<'a>(&'a self, text: &'a str) -> impl Iterator<Item = Match> + 'a {
        let mut start = 0;
        self.clause_boundary
            .find_iter(text)
            .map(|separator| separator.range())
            .chain(std::iter::once(text.len()..text.len()))
            .filter_map(move |separator| {
                let clause = &text[start..separator.start];
                let leading = clause.len() - clause.trim_start().len();
                let offset = start + leading;
                start = separator.end;
                let sentence = clause.trim();
                if self.uncertain.is_match(sentence) {
                    return None;
                }
                let (captures, rule_id) = match self.relationship.captures(sentence) {
                    Some(captures) => (captures, "explicit_relationship"),
                    None => (
                        self.event_relationship.captures(sentence)?,
                        "event_relationship",
                    ),
                };
                let subject = captures.name("subject")?;
                let predicate = captures.name("predicate")?;
                let object = captures.name("object")?;
                // A compound clause and a coordinated noun phrase can start identically.
                if self.coordinator.find(object.as_str()).is_some_and(|join| {
                    !self
                        .simple_noun_tail
                        .is_match(&object.as_str()[join.start()..])
                }) {
                    return None;
                }
                Some(Match {
                    range: offset..offset + sentence.len(),
                    rule_id,
                    value: CandidateValue::Relationship {
                        subject: subject.as_str().to_owned(),
                        predicate: predicate.as_str().to_owned(),
                        object: object.as_str().to_owned(),
                    },
                    argument_ranges: Some([
                        offset + subject.start()..offset + subject.end(),
                        offset + predicate.start()..offset + predicate.end(),
                        offset + object.start()..offset + object.end(),
                    ]),
                })
            })
    }
}

pub(crate) struct Match {
    pub(crate) range: Range<usize>,
    pub(crate) rule_id: &'static str,
    pub(crate) value: CandidateValue,
    pub(crate) argument_ranges: Option<[Range<usize>; 3]>,
}

fn calendar_date(date: Date, range: Range<usize>, rule_id: &'static str) -> Option<Match> {
    let iso_date = date
        .format(format_description!("[year]-[month]-[day]"))
        .ok()?;
    Some(Match {
        range,
        rule_id,
        value: CandidateValue::Date { iso_date },
        argument_ranges: None,
    })
}

fn scale_decimal(value: &str, scale: &str) -> Option<String> {
    let places = match scale.to_ascii_lowercase().as_str() {
        "thousand" => 3,
        "million" => 6,
        "billion" => 9,
        "trillion" => 12,
        _ => return None,
    };
    let (sign, unsigned) = value.split_at(usize::from(value.starts_with(['+', '-'])));
    let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let mut digits = format!("{integer}{fraction}");
    let decimal_at = integer.len() + places;
    if digits.len() < decimal_at {
        digits.push_str(&"0".repeat(decimal_at - digits.len()));
    }
    let (integer, fraction) = digits.split_at(decimal_at);
    let integer = integer.trim_start_matches('0');
    let integer = if integer.is_empty() { "0" } else { integer };
    if fraction.is_empty() {
        Some(format!("{sign}{integer}"))
    } else {
        Some(format!("{sign}{integer}.{fraction}"))
    }
}

fn ordinal_suffix(day: u8) -> &'static str {
    if (11..=13).contains(&day) {
        return "th";
    }
    match day % 10 {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    }
}

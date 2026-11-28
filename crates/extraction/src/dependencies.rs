//! Offline comparison of surface rules with externally annotated syntax.

use std::{error::Error, fmt, ops::Range};

use regex::Regex;
use serde::Deserialize;

use crate::{
    CandidateValue, DocumentVersion, EvidenceError,
    candidates::{MAX_CANDIDATES, Match},
};

/// A versioned dependency parse of an exact normalized document.
#[derive(Debug, Deserialize)]
pub struct DependencyParse {
    parser: String,
    normalized_text_sha256: String,
    sentences: Vec<Vec<Token>>,
}

impl DependencyParse {
    pub(crate) fn parser(&self) -> &str {
        &self.parser
    }

    pub(crate) fn relationships(
        &self,
        document: &DocumentVersion,
        uncertain: &Regex,
    ) -> Result<Vec<Match>, DependencyError> {
        if self.parser.trim().is_empty() || self.parser.len() > 256 {
            return Err(DependencyError::InvalidParser);
        }
        if self.normalized_text_sha256 != document.normalized_text_sha256() {
            return Err(DependencyError::WrongDocument);
        }

        let text = document.text();
        let mut matches = Vec::new();
        let mut cursor = 0;
        for tokens in &self.sentences {
            let tree = Sentence::from_tokens(tokens, text, &mut cursor)?;
            if matches.len() <= MAX_CANDIDATES
                && let Some(candidate) = tree.relationship(uncertain)
            {
                matches.push(candidate);
            }
        }
        if !text[cursor..].trim().is_empty() {
            return Err(DependencyError::InvalidTokenRange);
        }
        Ok(matches)
    }
}

/// Why a dependency parse cannot supply document evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyError {
    /// A short parser and model identifier is required.
    InvalidParser,
    /// The normalized document digest differs from the parse's digest.
    WrongDocument,
    /// Token byte ranges do not cover the document in order.
    InvalidTokenRange,
    /// Each sentence must have one root and valid, acyclic heads.
    InvalidTree,
    /// An emitted range could not be converted into evidence.
    Evidence(EvidenceError),
}

impl fmt::Display for DependencyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidParser => {
                f.write_str("dependency parse requires a short parser identifier")
            }
            Self::WrongDocument => {
                f.write_str("dependency parse does not match the normalized text hash")
            }
            Self::InvalidTokenRange => {
                f.write_str("dependency token ranges do not cover the normalized text")
            }
            Self::InvalidTree => {
                f.write_str("dependency sentence does not form a single rooted tree")
            }
            Self::Evidence(error) => error.fmt(f),
        }
    }
}

impl Error for DependencyError {}

#[derive(Debug, Deserialize)]
struct Token {
    start_byte: usize,
    end_byte: usize,
    head: Option<usize>,
    pos: String,
    relation: String,
}

struct Sentence<'a> {
    tokens: &'a [Token],
    text: &'a str,
    children: Vec<Vec<usize>>,
    root: usize,
}

impl<'a> Sentence<'a> {
    fn from_tokens(
        tokens: &'a [Token],
        text: &'a str,
        cursor: &mut usize,
    ) -> Result<Self, DependencyError> {
        let mut children = vec![Vec::new(); tokens.len()];
        let mut root = None;
        for (index, token) in tokens.iter().enumerate() {
            let gap = text.get(*cursor..token.start_byte);
            let word = text.get(token.start_byte..token.end_byte);
            if !gap.is_some_and(|gap| gap.trim().is_empty())
                || !word.is_some_and(|word| !word.trim().is_empty())
            {
                return Err(DependencyError::InvalidTokenRange);
            }
            *cursor = token.end_byte;
            if let Some(head) = token.head {
                children
                    .get_mut(head)
                    .ok_or(DependencyError::InvalidTree)?
                    .push(index);
            } else if root.replace(index).is_some() {
                return Err(DependencyError::InvalidTree);
            }
        }
        let root = root.ok_or(DependencyError::InvalidTree)?;
        let mut pending = vec![root];
        let mut reached = 0;
        while let Some(node) = pending.pop() {
            reached += 1;
            pending.extend(&children[node]);
        }
        // Each node has one head, so any cycle is disconnected from the root.
        if reached != tokens.len() {
            return Err(DependencyError::InvalidTree);
        }
        Ok(Self {
            tokens,
            text,
            children,
            root,
        })
    }

    fn relationship(&self, uncertain: &Regex) -> Option<Match> {
        let start = self.tokens.first()?.start_byte;
        let last = self.tokens.last()?;
        let quote = &self.text[start..last.end_byte];
        if quote.contains('?')
            || uncertain.is_match(quote)
            || self.tokens.iter().any(|token| {
                let word = self.word(token).to_ascii_lowercase();
                token.relation == "neg"
                    || matches!(word.as_str(), "no" | "neither" | "only" | "n't" | "n’t")
                    || (token.relation == "cc" && matches!(word.as_str(), "or" | "nor"))
                    || (token.relation == "aux"
                        && matches!(
                            word.as_str(),
                            "can" | "must" | "shall" | "will" | "'ll" | "’ll"
                        ))
            })
        {
            return None;
        }

        let root = &self.tokens[self.root];
        let (object_labels, rule_id): (&[&str], _) = match root.pos.as_str() {
            "VERB" => (&["dobj", "obj"], "dependency_transitive_verb"),
            "AUX"
                if matches!(
                    self.word(root).to_ascii_lowercase().as_str(),
                    "am" | "are" | "is" | "was" | "were"
                ) =>
            {
                (&["attr", "acomp"], "dependency_copular_complement")
            }
            _ => return None,
        };
        let mut subject = None;
        let mut object = None;
        let mut predicate = vec![self.root];
        for &index in &self.children[self.root] {
            let relation = self.tokens[index].relation.as_str();
            match relation {
                "nsubj" if subject.is_none() => subject = Some(index),
                relation if object_labels.contains(&relation) && object.is_none() => {
                    object = Some(index);
                }
                "aux" | "prt" | "compound:prt" if self.children[index].is_empty() => {
                    predicate.push(index);
                }
                "punct" | "cc" | "conj" => {}
                // Other attachments can qualify the relation or change its argument roles.
                _ => return None,
            }
        }
        let subject = self.nominal(subject?)?;
        let object = self.nominal(object?)?;
        let predicate = self.contiguous(&mut predicate)?;
        if subject.end > predicate.start || predicate.end > object.start {
            return None;
        }
        let end = if object.end <= last.start_byte
            && last.pos == "PUNCT"
            && matches!(self.word(last), "." | "!")
        {
            last.start_byte
        } else {
            last.end_byte
        };
        let quote = self.text[start..end].trim_end();
        Some(Match {
            range: start..start + quote.len(),
            rule_id,
            value: CandidateValue::Relationship {
                subject: self.text[subject.clone()].to_owned(),
                predicate: self.text[predicate.clone()].to_owned(),
                object: self.text[object.clone()].to_owned(),
            },
            argument_ranges: Some([subject, predicate, object]),
        })
    }

    fn nominal(&self, root: usize) -> Option<Range<usize>> {
        if !matches!(
            self.tokens[root].pos.as_str(),
            "NOUN" | "PROPN" | "PRON" | "ADJ" | "NUM"
        ) {
            return None;
        }
        let mut pending = vec![root];
        let mut indices = Vec::new();
        while let Some(node) = pending.pop() {
            indices.push(node);
            for &child in &self.children[node] {
                let token = &self.tokens[child];
                match token.relation.as_str() {
                    "det" | "amod" | "compound" | "flat" | "fixed" | "nummod" | "case" | "prep"
                    | "pobj" | "poss" | "nmod" | "nmod:poss" | "cc" => pending.push(child),
                    "conj"
                        if matches!(
                            token.pos.as_str(),
                            "NOUN" | "PROPN" | "ADJ" | "PRON" | "NUM"
                        ) =>
                    {
                        pending.push(child);
                    }
                    _ => return None,
                }
            }
        }
        self.contiguous(&mut indices)
    }

    fn contiguous(&self, indices: &mut [usize]) -> Option<Range<usize>> {
        indices.sort_unstable();
        let (&first, &last) = (indices.first()?, indices.last()?);
        (indices.len() == last - first + 1)
            .then(|| self.tokens[first].start_byte..self.tokens[last].end_byte)
    }

    fn word(&self, token: &Token) -> &str {
        &self.text[token.start_byte..token.end_byte]
    }
}

//! A small YAML document tree with source positions.
//!
//! deej's configuration is read through this tree instead of a serde deserializer, for two reasons:
//!
//! - The original deej resolved values the way Go's yaml.v2 + viper do (see [`crate::config::resolve`]), which
//!   needs each scalar's raw text and quoting style rather than a value already typed by a YAML 1.2 library.
//! - Saving settings edits the user's file in place, so every node keeps its byte range in the source.
//!
//! Duplicate keys are kept in the tree (the original deej silently used the last one, and the writer needs to
//! see all of them). Lookups go through [`Node::get`], which applies the same last-wins rule.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;

use saphyr_parser::{Event, Parser, ScalarStyle, Span, SpannedEventReceiver};

/// How a scalar was written in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Unquoted. Only plain scalars are subject to type resolution (`yes`, `010`, `~`, ...).
    Plain,
    /// Single or double quoted, or a `|`/`>` block scalar. Always a string.
    Quoted,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    Scalar {
        value: String,
        style: Style,
        tag: Option<String>,
    },
    Seq(Vec<Node>),
    /// Entries in source order, duplicates included.
    Map(Vec<(Node, Node)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: NodeKind,
    /// Byte range in the source the tree was parsed from.
    pub span: Range<usize>,
    /// 1-based line of the node's first character.
    pub line: usize,
}

/// A YAML syntax error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}, column {column}: {message}")]
pub struct SyntaxError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl Node {
    fn empty_map() -> Node {
        Node { kind: NodeKind::Map(Vec::new()), span: 0..0, line: 1 }
    }

    pub fn as_map(&self) -> Option<&[(Node, Node)]> {
        match &self.kind {
            NodeKind::Map(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn as_scalar(&self) -> Option<(&str, Style)> {
        match &self.kind {
            NodeKind::Scalar { value, style, .. } => Some((value, *style)),
            _ => None,
        }
    }

    /// Looks up a key in a mapping the way viper does: case-insensitively, last occurrence wins.
    /// Only plain or quoted scalar keys can match.
    pub fn get(&self, key: &str) -> Option<&Node> {
        self.as_map()?
            .iter()
            .rev()
            .find(|(k, _)| k.as_scalar().is_some_and(|(text, _)| text.eq_ignore_ascii_case(key)))
            .map(|(_, v)| v)
    }

    /// The keys of a mapping that occur more than once (compared case-insensitively), with every line they're on.
    pub fn duplicate_keys(&self) -> Vec<(String, Vec<usize>)> {
        let Some(entries) = self.as_map() else { return Vec::new() };

        let mut seen: Vec<(String, Vec<usize>)> = Vec::new();
        for (key, _) in entries {
            let Some((text, _)) = key.as_scalar() else { continue };
            let lower = text.to_lowercase();
            match seen.iter_mut().find(|(k, _)| *k == lower) {
                Some((_, lines)) => lines.push(key.line),
                None => seen.push((lower, vec![key.line])),
            }
        }

        seen.retain(|(_, lines)| lines.len() > 1);
        seen
    }
}

/// Parses the first YAML document in `source`. An empty document yields an empty mapping.
/// A leading UTF-8 byte order mark is skipped (the original deej accepted one).
pub fn parse(source: &str) -> Result<Node, SyntaxError> {
    let bom = if source.starts_with('\u{feff}') { '\u{feff}'.len_utf8() } else { 0 };
    let text = &source[bom..];

    let mut builder = TreeBuilder::new(text, bom);
    let mut parser = Parser::new_from_str(text);

    parser.load(&mut builder, false).map_err(|err| SyntaxError {
        message: err.info().to_string(),
        line: err.marker().line(),
        column: err.marker().col() + 1,
    })?;

    Ok(builder.root.unwrap_or_else(Node::empty_map))
}

/// Builds a [`Node`] tree from parser events.
struct TreeBuilder {
    /// Byte offset of each char index in the parsed text, plus one entry for the end.
    byte_offsets: Vec<usize>,
    /// Added to every offset, so spans are relative to the original source (before the BOM was skipped).
    base: usize,
    stack: Vec<Frame>,
    anchors: HashMap<usize, Node>,
    root: Option<Node>,
}

enum Frame {
    Seq { items: Vec<Node>, start: usize, line: usize, anchor: usize },
    Map { entries: Vec<(Node, Node)>, pending_key: Option<Node>, start: usize, line: usize, anchor: usize },
}

impl TreeBuilder {
    fn new(text: &str, base: usize) -> Self {
        let mut byte_offsets: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        byte_offsets.push(text.len());

        TreeBuilder { byte_offsets, base, stack: Vec::new(), anchors: HashMap::new(), root: None }
    }

    fn byte(&self, char_index: usize) -> usize {
        let last = self.byte_offsets.len() - 1;
        self.base + self.byte_offsets[char_index.min(last)]
    }

    fn push_node(&mut self, node: Node) {
        match self.stack.last_mut() {
            None => self.root = Some(node),
            Some(Frame::Seq { items, .. }) => items.push(node),
            Some(Frame::Map { entries, pending_key, .. }) => match pending_key.take() {
                None => *pending_key = Some(node),
                Some(key) => entries.push((key, node)),
            },
        }
    }

    fn remember(&mut self, anchor: usize, node: &Node) {
        if anchor != 0 {
            self.anchors.insert(anchor, node.clone());
        }
    }
}

fn tag_string(tag: Option<Cow<'_, saphyr_parser::Tag>>) -> Option<String> {
    tag.map(|t| format!("{}{}", t.handle, t.suffix))
}

impl<'input> SpannedEventReceiver<'input> for TreeBuilder {
    fn on_event(&mut self, event: Event<'input>, span: Span) {
        let start = self.byte(span.start.index());
        let end = self.byte(span.end.index());
        let line = span.start.line();

        match event {
            Event::Scalar(value, style, anchor, tag) => {
                let style = if style == ScalarStyle::Plain { Style::Plain } else { Style::Quoted };
                let node = Node {
                    kind: NodeKind::Scalar { value: value.into_owned(), style, tag: tag_string(tag) },
                    span: start..end,
                    line,
                };
                self.remember(anchor, &node);
                self.push_node(node);
            }
            Event::SequenceStart(anchor, _) => {
                self.stack.push(Frame::Seq { items: Vec::new(), start, line, anchor });
            }
            Event::MappingStart(anchor, _) => {
                self.stack.push(Frame::Map { entries: Vec::new(), pending_key: None, start, line, anchor });
            }
            Event::SequenceEnd => {
                if let Some(Frame::Seq { items, start, line, anchor }) = self.stack.pop() {
                    let node = Node { kind: NodeKind::Seq(items), span: start..end.max(start), line };
                    self.remember(anchor, &node);
                    self.push_node(node);
                }
            }
            Event::MappingEnd => {
                if let Some(Frame::Map { entries, start, line, anchor, .. }) = self.stack.pop() {
                    let node = Node { kind: NodeKind::Map(entries), span: start..end.max(start), line };
                    self.remember(anchor, &node);
                    self.push_node(node);
                }
            }
            Event::Alias(anchor) => {
                if let Some(node) = self.anchors.get(&anchor).cloned() {
                    self.push_node(node);
                }
            }
            Event::Nothing | Event::StreamStart | Event::StreamEnd | Event::DocumentStart(_) | Event::DocumentEnd => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar(node: &Node) -> &str {
        node.as_scalar().unwrap().0
    }

    #[test]
    fn spans_are_byte_ranges_of_the_original_source() {
        let src = "\u{feff}a: \"Réaltek (x)\"\nb: [1, 2]\n";
        let root = parse(src).unwrap();

        let a = root.get("a").unwrap();
        assert_eq!(&src[a.span.clone()], "\"Réaltek (x)\"");
        assert_eq!(a.as_scalar(), Some(("Réaltek (x)", Style::Quoted)));

        let b = root.get("b").unwrap();
        assert_eq!(b.line, 2);
        assert!(matches!(&b.kind, NodeKind::Seq(items) if items.len() == 2));
    }

    #[test]
    fn duplicate_keys_are_kept_and_last_wins() {
        let root = parse("x: 1\nX: 2\ny: 3\nx: 4\n").unwrap();

        assert_eq!(scalar(root.get("x").unwrap()), "4");
        assert_eq!(root.duplicate_keys(), vec![("x".to_string(), vec![1, 2, 4])]);
    }

    #[test]
    fn empty_and_null_values() {
        assert_eq!(parse("").unwrap(), Node::empty_map());

        let root = parse("a:\nb: ~\n").unwrap();
        for key in ["a", "b"] {
            let (_, style) = root.get(key).unwrap().as_scalar().unwrap();
            assert_eq!(style, Style::Plain);
        }
    }

    #[test]
    fn aliases_resolve_to_a_copy_of_the_anchor() {
        let root = parse("a: &x chrome.exe\nb: *x\n").unwrap();
        assert_eq!(scalar(root.get("b").unwrap()), "chrome.exe");
    }

    #[test]
    fn syntax_errors_report_their_position() {
        let err = parse("slider_mapping:\n  0: master\n\tbad: tab\n").unwrap_err();
        assert!(err.line >= 2, "{err}");
    }
}

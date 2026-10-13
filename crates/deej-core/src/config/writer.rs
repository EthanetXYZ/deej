//! Changing `config.yaml` without disturbing the rest of it.
//!
//! People edit this file by hand, and the default one is mostly comments. So edits are applied to the text, not by
//! re-serializing: only the lines of the edited keys change. Comments, blank lines, key order, quoting, line
//! endings and a BOM all survive. A scalar changed in place even keeps the comment at the end of its line.
//!
//! Every edit is verified: the result is parsed again, and must contain exactly the intended values at the edited
//! paths and be identical everywhere else. Otherwise nothing is written (ARCHITECTURE.md D4).

use std::fmt::Write as _;

use super::resolve::{self, Scalar};
use crate::yaml::{self, Node, NodeKind, Style};

/// A value deej writes.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// Written as a block list (`[]` when empty).
    List(Vec<String>),
}

/// Set (`Some`) or remove (`None`) the value at a key path, e.g. `["slider_sensitivity", "1", "curve"]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub path: Vec<String>,
    pub value: Option<Value>,
}

impl Edit {
    pub fn set(path: &[&str], value: Value) -> Edit {
        Edit { path: path.iter().map(|s| s.to_string()).collect(), value: Some(value) }
    }

    pub fn remove(path: &[&str]) -> Edit {
        Edit { path: path.iter().map(|s| s.to_string()).collect(), value: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    #[error("config.yaml can't be edited because it isn't valid YAML ({0})")]
    Syntax(#[from] yaml::SyntaxError),
    #[error("config.yaml must contain key: value settings at the top level")]
    NotAMapping,
    #[error("{0} can't be changed because it isn't a section of settings in config.yaml")]
    NotASection(String),
    #[error("the edit to {0} didn't come out as intended, so config.yaml was left unchanged")]
    VerificationFailed(String),
}

/// Applies `edits` in order and returns the new file contents.
pub fn apply_edits(source: &str, edits: &[Edit]) -> Result<String, WriteError> {
    let original = yaml::parse(source)?;
    if original.as_map().is_none() {
        return Err(WriteError::NotAMapping);
    }

    let mut text = source.to_string();
    for edit in edits {
        text = apply_one(&text, edit)?;
    }

    verify(&original, &text, edits)?;
    Ok(text)
}

// ---- locating things in the text ---------------------------------------------------------------------------

/// A key's text as deej's loader sees it (`010` is "8", like the original).
fn key_text(key: &Node) -> Option<String> {
    resolve::to_string(key)
}

/// Finds the last entry for `key` in a mapping (the one the loader uses).
fn find_entry<'a>(map: &'a Node, key: &str) -> Option<&'a (Node, Node)> {
    map.as_map()?.iter().rev().find(|(k, _)| key_text(k).is_some_and(|t| t.eq_ignore_ascii_case(key)))
}

/// Start of the line containing `at`. The first line starts after a byte order mark, which isn't content.
fn line_start(text: &str, at: usize) -> usize {
    let bom = if text.starts_with('\u{feff}') { '\u{feff}'.len_utf8() } else { 0 };
    text[..at].rfind('\n').map_or(bom, |i| i + 1)
}

/// Position just after the line containing `at` (after its `\n`, or the end of the text).
fn line_end(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |i| at + i + 1)
}

/// The last byte that belongs to a value: its own span for scalars and flow collections, the last descendant
/// for block collections (whose end span reaches into whatever follows).
fn value_last_byte(node: &Node, text: &str) -> usize {
    let flow =
        matches!(node.kind, NodeKind::Seq(_) | NodeKind::Map(_)) && text[node.span.start..].starts_with(['[', '{']);

    match &node.kind {
        NodeKind::Scalar { .. } => node.span.end.max(node.span.start),
        _ if flow => node.span.end,
        NodeKind::Seq(items) => items.iter().map(|i| value_last_byte(i, text)).max().unwrap_or(node.span.start),
        NodeKind::Map(entries) => entries
            .iter()
            .map(|(k, v)| value_last_byte(k, text).max(value_last_byte(v, text)))
            .max()
            .unwrap_or(node.span.start),
    }
}

/// The byte range of a whole entry: from the start of the key's line to the end of the value's last line.
fn entry_range(text: &str, key: &Node, value: &Node) -> std::ops::Range<usize> {
    let start = line_start(text, key.span.start);
    let last = value_last_byte(value, text).max(key.span.end);
    // a zero-length null value can sit at the start of the next line; it belongs to the key's line
    let last = if last > key.span.end && text[key.span.end..last].trim().is_empty() { key.span.end } else { last };
    start..line_end(text, last.saturating_sub(1).max(key.span.start))
}

fn indent_of(text: &str, node: &Node) -> usize {
    node.span.start - line_start(text, node.span.start)
}

fn is_flow(text: &str, node: &Node) -> bool {
    matches!(node.kind, NodeKind::Seq(_) | NodeKind::Map(_)) && text[node.span.start..].starts_with(['[', '{'])
}

fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

// ---- rendering ---------------------------------------------------------------------------------------------

/// A scalar as YAML text: plain when it reads back as the same string, double-quoted otherwise.
pub fn render_scalar(value: &Value) -> String {
    match value {
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => resolve::format_go_float(*f),
        Value::Str(s) => render_string(s),
        Value::List(items) if items.is_empty() => "[]".to_string(),
        Value::List(_) => unreachable!("lists are rendered as blocks"),
    }
}

/// A key: slider indexes stay plain numbers, as people write them (`1:`, not `"1":`).
fn render_key(key: &str) -> String {
    if !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit()) { key.to_string() } else { render_string(key) }
}

fn render_string(s: &str) -> String {
    let plain_ok = !s.is_empty()
        && s.trim() == s
        && !s.starts_with([
            '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`', '~',
        ])
        && !s.contains(": ")
        && !s.contains(" #")
        && !s.ends_with(':')
        && !s.chars().any(|c| c.is_control())
        && resolve::resolve(s, Style::Plain, None) == Scalar::Str(s.to_string());

    if plain_ok {
        return s.to_string();
    }

    let mut quoted = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(quoted, "\\u{:04x}", c as u32);
            }
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// What to write for a key: a value, or a section of further keys (when creating missing sections).
enum Render {
    Value(Value),
    Section(String, Box<Render>),
}

fn render_entry(out: &mut String, indent: usize, key: &str, render: &Render, nl: &str) {
    let pad = " ".repeat(indent);
    match render {
        Render::Value(Value::List(items)) if !items.is_empty() => {
            let _ = write!(out, "{pad}{}:{nl}", render_key(key));
            for item in items {
                let _ = write!(out, "{pad}  - {}{nl}", render_string(item));
            }
        }
        Render::Value(value) => {
            let _ = write!(out, "{pad}{}: {}{nl}", render_key(key), render_scalar(value));
        }
        Render::Section(child, inner) => {
            let _ = write!(out, "{pad}{}:{nl}", render_key(key));
            render_entry(out, indent + 2, child, inner, nl);
        }
    }
}

/// Re-renders an existing node as block YAML (used to turn a flow mapping into a block one before adding to it).
fn render_existing(out: &mut String, indent: usize, key: &str, node: &Node, nl: &str) {
    let pad = " ".repeat(indent);
    match &node.kind {
        NodeKind::Scalar { value, style, .. } => {
            let text = if *style == Style::Plain { value.clone() } else { render_string(value) };
            let _ = write!(out, "{pad}{}: {text}{nl}", render_key(key));
        }
        NodeKind::Seq(items) => {
            if items.is_empty() {
                let _ = write!(out, "{pad}{}: []{nl}", render_key(key));
                return;
            }
            let _ = write!(out, "{pad}{}:{nl}", render_key(key));
            for item in items {
                let text = match &item.kind {
                    NodeKind::Scalar { value, style: Style::Plain, .. } => value.clone(),
                    NodeKind::Scalar { value, .. } => render_string(value),
                    _ => String::new(),
                };
                let _ = write!(out, "{pad}  - {text}{nl}");
            }
        }
        NodeKind::Map(entries) => {
            if entries.is_empty() {
                let _ = write!(out, "{pad}{}: {{}}{nl}", render_key(key));
                return;
            }
            let _ = write!(out, "{pad}{}:{nl}", render_key(key));
            for (k, v) in entries {
                render_existing(out, indent + 2, &key_text(k).unwrap_or_default(), v, nl);
            }
        }
    }
}

// ---- edits -------------------------------------------------------------------------------------------------

fn apply_one(text: &str, edit: &Edit) -> Result<String, WriteError> {
    let root = yaml::parse(text)?;
    match &edit.value {
        Some(value) => set(text, &root, &edit.path, value),
        None => Ok(remove(text, &root, &edit.path)?.unwrap_or_else(|| text.to_string())),
    }
}

/// Walks to the deepest existing section along `path`. Returns that node and how many path parts it covers.
fn deepest<'a>(root: &'a Node, path: &[String]) -> (&'a Node, Option<&'a (Node, Node)>, usize) {
    let mut node = root;
    let mut entry = None;
    let mut depth = 0;

    for part in path {
        match node.as_map().and(find_entry(node, part)) {
            Some(found) => {
                entry = Some(found);
                node = &found.1;
                depth += 1;
            }
            None => break,
        }
    }

    (node, entry, depth)
}

fn set(text: &str, root: &Node, path: &[String], value: &Value) -> Result<String, WriteError> {
    let nl = newline(text);
    let (node, entry, depth) = deepest(root, path);

    // the key exists: replace its value
    if depth == path.len() {
        let (key, old) = entry.expect("depth > 0");
        let in_place = matches!(&old.kind, NodeKind::Scalar { style, .. } if *style == Style::Plain || text[old.span.start..].starts_with(['"', '\'']))
            && !old.span.is_empty()
            && !resolve::is_null(old)
            && line_start(text, old.span.start) == line_start(text, key.span.start)
            && !matches!(value, Value::List(items) if !items.is_empty());

        if in_place {
            return Ok(format!("{}{}{}", &text[..old.span.start], render_scalar(value), &text[old.span.end..]));
        }

        let range = entry_range(text, key, old);
        let mut rendered = String::new();
        render_entry(&mut rendered, indent_of(text, key), path.last().unwrap(), &Render::Value(value.clone()), nl);
        return Ok(format!("{}{}{}", &text[..range.start], rendered, &text[range.end..]));
    }

    // what's missing below the deepest existing section
    let missing = &path[depth..];
    let mut render = Render::Value(value.clone());
    for part in missing[1..].iter().rev() {
        render = Render::Section(part.clone(), Box::new(render));
    }
    let first = &missing[0];

    match &node.kind {
        // add to an existing block section
        NodeKind::Map(entries) if !entries.is_empty() && !is_flow(text, node) => {
            let indent = indent_of(text, &entries[0].0);

            // slider indexes stay in numeric order
            let before = first.parse::<i64>().ok().and_then(|n| {
                entries.iter().find(|(k, _)| key_text(k).and_then(|t| t.parse::<i64>().ok()).is_some_and(|m| m > n))
            });

            let at = match before {
                Some((k, _)) => line_start(text, k.span.start),
                None => {
                    let (k, v) = entries.last().unwrap();
                    entry_range(text, k, v).end
                }
            };

            let mut rendered = String::new();
            if at == text.len() && !text.is_empty() && !text.ends_with('\n') {
                rendered.push_str(nl);
            }
            // a new top-level section gets a blank line before it, like the rest of the file
            if depth == 0
                && before.is_none()
                && !text[..at].trim_end_matches(['\r', '\n']).is_empty()
                && !text[..at].ends_with(&format!("{nl}{nl}"))
            {
                rendered.push_str(nl);
            }
            render_entry(&mut rendered, indent, first, &render, nl);
            Ok(format!("{}{}{}", &text[..at], rendered, &text[at..]))
        }

        // the document itself is empty (or only comments): append
        NodeKind::Map(entries) if depth == 0 && entries.is_empty() => {
            let mut out = text.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push_str(nl);
            }
            render_entry(&mut out, 0, first, &render, nl);
            Ok(out)
        }

        // an empty, null or flow section: rewrite that entry as a block section with the new key added
        NodeKind::Map(_) | NodeKind::Scalar { .. }
            if depth > 0 && (resolve::is_null(node) || matches!(node.kind, NodeKind::Map(_))) =>
        {
            let (key, old) = entry.unwrap();
            let range = entry_range(text, key, old);
            let indent = indent_of(text, key);
            let pad = " ".repeat(indent);

            let mut rendered = String::new();
            let _ = write!(rendered, "{pad}{}:{nl}", render_key(&path[depth - 1]));
            if let NodeKind::Map(entries) = &old.kind {
                for (k, v) in entries {
                    render_existing(&mut rendered, indent + 2, &key_text(k).unwrap_or_default(), v, nl);
                }
            }
            render_entry(&mut rendered, indent + 2, first, &render, nl);
            Ok(format!("{}{}{}", &text[..range.start], rendered, &text[range.end..]))
        }

        _ => Err(WriteError::NotASection(path[..depth].join("."))),
    }
}

/// Removes the key at `path`, and any section that removal leaves empty (except `slider_mapping`, which becomes
/// `{}` so the comment block above it keeps making sense). Returns `None` if the key isn't there.
fn remove(text: &str, root: &Node, path: &[String]) -> Result<Option<String>, WriteError> {
    let (_, entry, depth) = deepest(root, path);
    if depth < path.len() {
        return Ok(None);
    }

    let (key, value) = entry.unwrap();

    if path.len() > 1 {
        let (parent, parent_entry, _) = deepest(root, &path[..path.len() - 1]);
        let siblings = parent.as_map().map_or(0, |e| e.len());

        // the last key of a flow section, or of a section: handle at the section's level
        if siblings == 1 {
            if path.len() == 2 && path[0].eq_ignore_ascii_case("slider_mapping") {
                let (pk, pv) = parent_entry.unwrap();
                let range = entry_range(text, pk, pv);
                let key_source = &text[pk.span.clone()];
                let rendered = format!("{}{key_source}: {{}}{}", " ".repeat(indent_of(text, pk)), newline(text));
                return Ok(Some(format!("{}{}{}", &text[..range.start], rendered, &text[range.end..])));
            }
            return remove(text, root, &path[..path.len() - 1]);
        }

        if is_flow(text, parent) {
            // rewrite the section as a block without this key
            let (pk, pv) = parent_entry.unwrap();
            let range = entry_range(text, pk, pv);
            let indent = indent_of(text, pk);
            let nl = newline(text);
            let mut rendered = String::new();
            let _ = write!(rendered, "{}{}:{nl}", " ".repeat(indent), render_key(&path[path.len() - 2]));
            for (k, v) in parent.as_map().unwrap() {
                if !std::ptr::eq(k, key) {
                    render_existing(&mut rendered, indent + 2, &key_text(k).unwrap_or_default(), v, nl);
                }
            }
            return Ok(Some(format!("{}{}{}", &text[..range.start], rendered, &text[range.end..])));
        }
    }

    let range = entry_range(text, key, value);
    Ok(Some(format!("{}{}", &text[..range.start], &text[range.end..])))
}

// ---- verification ------------------------------------------------------------------------------------------

/// The parts of a document that matter, without positions.
#[derive(Debug, Clone, PartialEq)]
enum Shape {
    Scalar(String, bool),
    Seq(Vec<Shape>),
    Map(Vec<(String, Shape)>),
}

fn shape(node: &Node) -> Shape {
    match &node.kind {
        NodeKind::Scalar { value, style, .. } => Shape::Scalar(value.clone(), *style == Style::Plain),
        NodeKind::Seq(items) => Shape::Seq(items.iter().map(shape).collect()),
        NodeKind::Map(entries) => Shape::Map(
            entries.iter().map(|(k, v)| (key_text(k).unwrap_or_default().to_lowercase(), shape(v))).collect(),
        ),
    }
}

/// Removes every edited path (and sections left empty by that) so the rest can be compared.
fn without(mut s: Shape, paths: &[Vec<String>]) -> Shape {
    for path in paths {
        strip(&mut s, path);
    }
    s
}

fn strip(s: &mut Shape, path: &[String]) -> bool {
    let Shape::Map(entries) = s else { return false };
    let key = path[0].to_lowercase();

    if path.len() == 1 {
        entries.retain(|(k, _)| *k != key);
    } else {
        for (_, child) in entries.iter_mut().filter(|(k, _)| *k == key) {
            strip(child, &path[1..]);
        }
        entries.retain(|(k, v)| {
            *k != key
                || !matches!(v, Shape::Map(m) if m.is_empty())
                    && !matches!(v, Shape::Scalar(t, true) if t.is_empty() || t == "~")
        });
    }
    entries.is_empty()
}

fn lookup<'a>(s: &'a Shape, path: &[String]) -> Option<&'a Shape> {
    let Shape::Map(entries) = s else { return None };
    let key = path[0].to_lowercase();
    let (_, child) = entries.iter().rev().find(|(k, _)| *k == key)?;
    if path.len() == 1 { Some(child) } else { lookup(child, &path[1..]) }
}

fn expected_shape(value: &Value) -> Shape {
    let scalar = |text: String| {
        let parsed = yaml::parse(&format!("x: {text}\n")).ok();
        parsed.and_then(|n| n.get("x").map(shape)).unwrap_or(Shape::Scalar(text, true))
    };

    match value {
        Value::List(items) if items.is_empty() => Shape::Seq(Vec::new()),
        Value::List(items) => Shape::Seq(items.iter().map(|i| scalar(render_string(i))).collect()),
        other => scalar(render_scalar(other)),
    }
}

fn verify(original: &Node, text: &str, edits: &[Edit]) -> Result<(), WriteError> {
    let result = yaml::parse(text).map_err(|_| WriteError::VerificationFailed("the file".into()))?;
    let (before, after) = (shape(original), shape(&result));

    // the last edit for each path decides its final value
    let mut paths: Vec<Vec<String>> = Vec::new();
    for edit in edits.iter().rev() {
        let path: Vec<String> = edit.path.iter().map(|p| p.to_lowercase()).collect();
        if paths.contains(&path) {
            continue;
        }
        let ok = match &edit.value {
            Some(value) => lookup(&after, &path) == Some(&expected_shape(value)),
            None => lookup(&after, &path).is_none(),
        };
        if !ok {
            return Err(WriteError::VerificationFailed(edit.path.join(".")));
        }
        paths.push(path);
    }

    if without(before, &paths) != without(after, &paths) {
        return Err(WriteError::VerificationFailed("the rest of the file".into()));
    }

    Ok(())
}

#[cfg(test)]
mod tests;

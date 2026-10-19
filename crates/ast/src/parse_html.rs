//! HTML string -> AST.
//!
//! Maud's `html!` is a `macro_rules!` macro: by the time it has run, the only
//! thing left is a `Markup` string builder. There is no way to intercept its
//! token stream, so "write Maud, get a reactive wasm view" cannot be done by
//! wrapping `html!`. What *is* possible — and what this module provides — is
//! the other direction: take the HTML Maud produced and recover a
//! [`Node`](crate::Node) tree from it.
//!
//! That gives two things:
//!
//! 1. **Round trip.** Parse -> [`Node`] -> [`render_html`] reproduces the input
//!    (modulo attribute order and void-tag spelling), which is what makes it
//!    safe to migrate an existing Maud codebase to the AST in stages.
//! 2. **A free structural diff.** A parsed tree can be walked and compared, so a
//!    view can be tested against the markup its endpoint actually emits.
//!
//! ## What survives, and what does not
//!
//! Preserved: the element tree, attribute names and values, text runs,
//! comments (as raw text), and the `data-velo-on` annotations
//! [`render_html`](crate::html) emits — so a server-rendered fragment can be
//! re-read into a tree whose event bindings line up with the client's.
//!
//! Not preserved: reactivity. A parsed expression is a constant string; the AST
//! has no way to know that Maud's `(row.name)` came from a database column.
//! Interactivity must come from [`crate::view::HandlerRef`] on the [`View`] you
//! ship alongside — which is exactly the split in "authored on a backend":
//! structure from Maud, behaviour from named handlers.
//!
//! ## Scope
//!
//! A focused, forgiving parser rather than a spec-complete HTML5 tree builder.
//! It handles void elements, raw-text elements, quoted and unquoted attribute
//! values, comments, and the implicit-close rules for the handful of elements
//! HTML permits without them (`p`, `li`, `dt`/`dd`, `tr`, `td`/`th`,
//! `option`, `thead`/`tbody`/`tfoot`). It does not implement the adoption agency
//! algorithm or foster parenting, so deeply malformed markup can nest
//! differently than a browser would. Everything Maud emits is well-formed, so
//! that is not a practical concern here.

use std::collections::BTreeMap;

use crate::attr::Attr;
use crate::event::{EventAction, EventBinding};
use crate::node::Node;
use crate::value::Value;
use crate::view::View;
use crate::attr::{is_raw_text_element, is_void_element};

/// The attribute `render_html` writes to mark an event binding. Preserved by
/// the parser so a re-read tree still lines up with the client's bindings.
pub const EVENT_ANNOTATION: &str = "data-velo-on";

/// The attribute `render_html` writes to mark a network-backed handler.
pub const NET_ANNOTATION: &str = "data-velo-net";

/// Where parsing failed.
#[derive(Debug, Clone, PartialEq)]
pub struct HtmlParseError {
    /// What went wrong.
    pub message: String,
    /// Byte offset into the input.
    pub offset: usize,
}

impl std::fmt::Display for HtmlParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for HtmlParseError {}

/// Parser options.
#[derive(Debug, Clone)]
pub struct ParseOptions {
    /// Wrap the result in a [`Node::Fragment`] so a multi-root document parses.
    /// On by default — Maud emits multiple top-level nodes routinely.
    pub allow_multiple_roots: bool,
    /// Re-emit `data-velo-on` / `data-velo-net` as plain attributes instead of
    /// converting them to [`EventBinding`]s with empty actions.
    pub keep_annotations_as_attributes: bool,
    /// Drop comment nodes instead of keeping their text.
    pub strip_comments: bool,
    /// Convert `<script type="application/json">` bodies into
    /// [`Node::Text`] with the JSON unescaped, so a seed island survives a
    /// round trip as data rather than as markup.
    pub parse_json_islands: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions {
            allow_multiple_roots: true,
            keep_annotations_as_attributes: false,
            strip_comments: false,
            parse_json_islands: true,
        }
    }
}

impl ParseOptions {
    /// Default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Emit annotations as ordinary attributes.
    pub fn raw_annotations(mut self) -> Self {
        self.keep_annotations_as_attributes = true;
        self
    }
}

/// Parse an HTML fragment into a [`Node`].
///
/// ```
/// use velo_ast::node::Node;
///
/// let node = velo_ast::parse_html::parse(r#"<p class="hi">Ada</p>"#).unwrap();
/// assert!(matches!(node, Node::Fragment(_)));
/// ```
pub fn parse(input: &str) -> Result<Node, HtmlParseError> {
    parse_with(input, &ParseOptions::default())
}

/// Parse with explicit options.
pub fn parse_with(input: &str, options: &ParseOptions) -> Result<Node, HtmlParseError> {
    let mut p = Parser {
        src: input.as_bytes(),
        text: input,
        pos: 0,
        options,
    };
    let roots = p.parse_nodes(None)?;
    p.skip_trailing();

    if roots.is_empty() {
        return Ok(Node::Fragment(vec![]));
    }
    if options.allow_multiple_roots {
        return Ok(Node::Fragment(roots));
    }
    if roots.len() > 1 {
        return Err(HtmlParseError {
            message: format!("expected a single root element, found {}", roots.len()),
            offset: 0,
        });
    }
    Ok(roots.into_iter().next().unwrap())
}

/// Parse an HTML fragment into a [`View`] named `name`.
///
/// The result has no signals and no handlers — a parsed tree is inert. Add
/// [`View::handler`] entries to make it interactive.
pub fn parse_view(input: &str, name: &str) -> Result<View, HtmlParseError> {
    Ok(View::new(name, parse(input)?))
}

/// Decode an HTML entity reference to the character(s) it denotes, if `input`
/// starts with one.
///
/// Numeric references decode to the codepoint they name; named references come
/// from the fixed table below, which covers everything Maud can emit. Anything
/// else is left alone, so an unknown entity survives a round trip verbatim
/// instead of being silently dropped.
pub fn unescape(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp + 1..];
        match tail.find(';') {
            // Bounded so a stray `&` followed by a long run of prose does not
            // scan the rest of the document looking for a semicolon.
            Some(k) if k <= 12 => match decode_entity(&tail[..k]) {
                Some(decoded) => {
                    out.push_str(&decoded);
                    rest = &tail[k + 1..];
                }
                None => {
                    out.push('&');
                    rest = tail;
                }
            },
            _ => {
                out.push('&');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_entity(name: &str) -> Option<String> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse::<u32>().ok()?,
        };
        return char::from_u32(code).map(|c| c.to_string());
    }
    Some(
        match name {
            "amp" => "&",
            "lt" => "<",
            "gt" => ">",
            "quot" => "\"",
            "apos" => "'",
            "nbsp" => "\u{a0}",
            "hellip" => "…",
            "mdash" => "—",
            "ndash" => "–",
            "rsquo" => "\u{2019}",
            "lsquo" => "\u{2018}",
            "rdquo" => "\u{201d}",
            "ldquo" => "\u{201c}",
            "times" => "\u{d7}",
            "copy" => "\u{a9}",
            "reg" => "\u{ae}",
            "trade" => "\u{2122}",
            _ => return None,
        }
        .to_string(),
    )
}

/// Elements whose start tag implicitly closes an open sibling, and the sibling
/// it closes. Maud lets authors omit these end tags, so the parser must too.
fn implicitly_closes(open_tag: &str, new_tag: &str) -> bool {
    match (open_tag, new_tag) {
        // A block element closes an open <p>.
        ("p", t) => matches!(
            t,
            "address" | "article" | "aside" | "blockquote" | "div" | "dl" | "fieldset"
                | "figcaption" | "figure" | "footer" | "form" | "h1" | "h2" | "h3" | "h4"
                | "h5" | "h6" | "header" | "hr" | "main" | "nav" | "ol" | "p" | "pre"
                | "section" | "table" | "ul"
        ),
        ("li", "li") => true,
        ("dt", "dt" | "dd") => true,
        ("dd", "dt" | "dd") => true,
        ("option", "option" | "optgroup") => true,
        ("optgroup", "optgroup") => true,
        ("tr", "tr") => true,
        ("td", "td" | "th" | "tr") => true,
        ("th", "td" | "th" | "tr") => true,
        ("thead", "tbody" | "tfoot") => true,
        ("tbody", "tbody" | "tfoot") => true,
        ("tfoot", "tbody") => true,
        ("head", "body") => true,
        ("html", "head" | "body") => true,
        _ => false,
    }
}

/// Do we need an explicit close tag for `tag`?
fn needs_close_tag(tag: &str) -> bool {
    !is_void_element(tag)
}

struct Parser<'a> {
    src: &'a [u8],
    text: &'a str,
    pos: usize,
    options: &'a ParseOptions,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn starts_with(&self, s: &str) -> bool {
        self.text[self.pos..].starts_with(s)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn skip_to(&mut self, needle: &str) {
        if let Some(idx) = self.text[self.pos..].find(needle) {
            self.pos += idx;
        } else {
            self.pos = self.src.len();
        }
    }

    /// Consume trailing whitespace and any stray close tags.
    fn skip_trailing(&mut self) {
        loop {
            self.skip_whitespace();
            if self.starts_with("</") {
                self.pos += 2;
                while matches!(self.peek(), Some(c) if c != b'>') {
                    self.pos += 1;
                }
                if self.peek() == Some(b'>') {
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
    }

    /// Parse siblings until `parent` is closed (or input ends at the top level).
    fn parse_nodes(&mut self, parent: Option<&str>) -> Result<Vec<Node>, HtmlParseError> {
        let mut out = Vec::new();
        loop {
            match self.peek() {
                None => break,
                Some(b'<') => {
                    if self.starts_with("</") {
                        let Some(p) = parent else { break };
                        let saved = self.pos;
                        self.pos += 2;
                        let name = self.read_tag_name();
                        self.skip_whitespace();
                        if self.peek() == Some(b'>') {
                            self.pos += 1;
                        }
                        if name.eq_ignore_ascii_case(p) {
                            break;
                        }
                        // A close tag for some other element: rewind and let the
                        // caller decide (it will unwind the stack).
                        self.pos = saved;
                        break;
                    }
                    if self.starts_with("<!--") {
                        out.push(self.parse_comment()?);
                        continue;
                    }
                    if self.starts_with("<!") {
                        // Doctype / CDATA / processing instruction.
                        self.pos += 2;
                        self.skip_to(">");
                        self.pos = (self.pos + 1).min(self.src.len());
                        continue;
                    }
                    // A start tag that HTML lets close its parent implicitly
                    // (`<li>a<li>b`, `<p>one<p>two`). Break without consuming so
                    // the caller re-reads this tag and the parent never sees an
                    // end tag that is not there.
                    if let Some(p) = parent {
                        if self.peek_is_implicit_close(p) {
                            break;
                        }
                    }
                    out.push(self.parse_element()?);
                }
                Some(_) => {
                    let text = self.parse_text_run();
                    if !text.is_empty() {
                        out.push(Node::text(unescape(&text)));
                    }
                }
            }
        }
        Ok(out)
    }

    /// Would the next start tag implicitly close `parent`?
    ///
    /// Peeks without consuming, so the caller can leave the tag in place for the
    /// enclosing level to parse.
    fn peek_is_implicit_close(&self, parent: &str) -> bool {
        let after = self.pos + 1;
        if after >= self.src.len() || self.src[after] == b'/' || self.src[after] == b'!' {
            return false;
        }
        let mut end = after;
        while end < self.src.len() {
            let c = self.src[end];
            if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b':' || c >= 0x80 {
                end += 1;
            } else {
                break;
            }
        }
        if end == after {
            return false;
        }
        let name = self.text[after..end].to_ascii_lowercase();
        implicitly_closes(parent, &name)
    }

    /// Case-insensitive search for `needle` starting at `from`.
    ///
    /// Done bytewise rather than by lowercasing the remaining input, which
    /// would make a document with many `<script>` blocks quadratic in its
    /// length.
    fn find_ci(&self, from: usize, needle: &str) -> Option<usize> {
        let n = needle.as_bytes();
        let last = self.src.len().checked_sub(n.len())?;
        let mut i = from;
        while i <= last {
            if self.src[i..i + n.len()]
                .iter()
                .zip(n)
                .all(|(a, b)| a.to_ascii_lowercase() == *b)
            {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    fn parse_comment(&mut self) -> Result<Node, HtmlParseError> {
        let start = self.pos;
        self.pos += 4; // `<!--`
        self.skip_to("-->");
        let end = (self.pos + 3).min(self.src.len());
        let body = self.text[start + 4..self.pos.min(self.src.len())].to_string();
        self.pos = end;
        if self.options.strip_comments {
            return Ok(Node::Missing);
        }
        Ok(Node::raw(format!("<!--{body}-->")))
    }

    /// Consume text up to the next `<`.
    fn parse_text_run(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c == b'<' {
                break;
            }
            self.pos += 1;
        }
        self.text[start..self.pos].to_string()
    }

    fn read_tag_name(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b':' || c >= 0x80 {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.text[start..self.pos].to_ascii_lowercase()
    }

    fn parse_element(&mut self) -> Result<Node, HtmlParseError> {
        let open_at = self.pos;
        self.pos += 1; // `<`
        let tag = self.read_tag_name();
        if tag.is_empty() {
            // A `<` with no tag name behind it is literal text, not markup
            // (`a < b`). Skip the `<` and keep going: a stray angle bracket in
            // prose must not fail the parse.
            self.pos = open_at + 1;
            return Ok(Node::Missing);
        }

        let mut attrs: Vec<Attr> = Vec::new();
        let mut events: Vec<EventBinding> = Vec::new();
        let mut annotations: Vec<(String, String)> = Vec::new();
        let mut self_closing = false;

        loop {
            self.skip_whitespace();
            match self.peek() {
                // End of input inside a start tag: recover by treating the tag
                // as closed. Failing here would make one truncated tag at the
                // end of a response fail the entire parse.
                None => break,
                Some(b'>') => {
                    self.pos += 1;
                    break;
                }
                Some(b'/') => {
                    self.pos += 1;
                    if self.peek() == Some(b'>') {
                        self.pos += 1;
                        self_closing = true;
                        break;
                    }
                    // Stray `/`: treat as whitespace.
                }
                Some(_) => {
                    let name = self.read_attr_name()?;
                    if name.is_empty() {
                        // Unparseable byte; skip it so one bad char cannot make
                        // the whole document unparseable.
                        self.pos += 1;
                        continue;
                    }
                    let value = if matches!(self.peek(), Some(b'=')) {
                        self.pos += 1;
                        self.read_attr_value()?
                    } else {
                        // Valueless attribute: HTML treats it as the empty
                        // string, which is exactly boolean-attribute semantics.
                        String::new()
                    };

                    let lname = name.to_ascii_lowercase();
                    if !self.options.keep_annotations_as_attributes && lname == EVENT_ANNOTATION {
                        if let Some(binding) = parse_event_annotation(&value) {
                            events.push(binding);
                            continue;
                        }
                    }
                    if !self.options.keep_annotations_as_attributes && lname == NET_ANNOTATION {
                        annotations.push((lname, value));
                        continue;
                    }
                    if is_boolean_attr(&lname) && value.is_empty() {
                        attrs.push(Attr::boolean(lname, crate::expr::Expr::Bool(true)));
                    } else {
                        attrs.push(Attr::stat(lname, unescape(&value)));
                    }
                }
            }
        }

        // A JSON seed island is data, not markup.
        if self.options.parse_json_islands && tag == "script" {
            if let Some(json) = self.find_attr(&attrs, "type") {
                if json == "application/json" {
                    let body = self.read_raw_text_body("script")?;
                    return Ok(Node::text(unescape(&body)));
                }
            }
        }

        let mut children = Vec::new();
        if is_raw_text_element(&tag) {
            let body = self.read_raw_text_body(&tag)?;
            if !body.is_empty() {
                children.push(Node::text(body));
            }
            return Ok(Node::Element(crate::node::Element {
                tag,
                attrs,
                events,
                children,
            }));
        }

        if !self_closing && needs_close_tag(&tag) {
            children = self.parse_nodes(Some(&tag))?;
            // Consume the matching `</tag>` when the caller is positioned on it.
            if self.starts_with("</") {
                let saved = self.pos;
                self.pos += 2;
                let close = self.read_tag_name();
                self.skip_whitespace();
                if self.peek() == Some(b'>') {
                    self.pos += 1;
                }
                if !close.eq_ignore_ascii_case(&tag) {
                    self.pos = saved;
                }
            }
        }

        let _ = open_at;
        Ok(Node::Element(crate::node::Element {
            tag,
            attrs,
            events,
            children,
        }))
    }

    fn find_attr(&self, attrs: &[Attr], name: &str) -> Option<String> {
        attrs.iter().find_map(|a| {
            if a.name == name {
                match &a.value {
                    crate::attr::AttrValue::Static(e) => e.literal(),
                    _ => None,
                }
            } else {
                None
            }
        })
    }

    /// Read the body of a raw-text element, consuming its closing tag.
    fn read_raw_text_body(&mut self, tag: &str) -> Result<String, HtmlParseError> {
        let start = self.pos;
        let close = format!("</{tag}");
        let end = self
            .find_ci(start, &close)
            .unwrap_or(self.src.len());
        let body = self.text[start..end].to_string();
        self.pos = end;
        if self.pos < self.src.len() {
            self.skip_to(">");
            self.pos = (self.pos + 1).min(self.src.len());
        }
        Ok(body)
    }

    fn read_attr_name(&mut self) -> Result<String, HtmlParseError> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() || c == b'=' || c == b'>' || c == b'/' || c == b'<' {
                break;
            }
            self.pos += 1;
        }
        Ok(self.text[start..self.pos].to_string())
    }

    fn read_attr_value(&mut self) -> Result<String, HtmlParseError> {
        self.skip_whitespace();
        match self.peek() {
            Some(q @ (b'"' | b'\'')) => {
                self.pos += 1;
                let start = self.pos;
                while let Some(c) = self.peek() {
                    if c == q {
                        break;
                    }
                    self.pos += 1;
                }
                let v = self.text[start..self.pos].to_string();
                // An unterminated quoted value is recovered, not rejected: the
                // text up to end-of-input is a better guess than failing to
                // parse a page because one attribute lost its closing quote.
                if self.peek().is_some() {
                    self.pos += 1;
                }
                Ok(v)
            }
            _ => {
                let start = self.pos;
                while let Some(c) = self.peek() {
                    if c.is_ascii_whitespace() || c == b'>' {
                        break;
                    }
                    self.pos += 1;
                }
                Ok(self.text[start..self.pos].to_string())
            }
        }
    }

}

/// `on*` and other HTML boolean attributes.
fn is_boolean_attr(name: &str) -> bool {
    matches!(
        name,
        "disabled"
            | "checked"
            | "selected"
            | "readonly"
            | "required"
            | "multiple"
            | "hidden"
            | "autofocus"
            | "novalidate"
            | "open"
            | "loop"
            | "muted"
            | "controls"
            | "reversed"
            | "async"
            | "defer"
            | "ismap"
            | "itemscope"
            | "default"
            | "playsinline"
            | "inert"
            | "formnovalidate"
    )
}

/// Recover an [`EventBinding`] from a `data-velo-on="click[0]"` annotation.
///
/// The op list is **not** recoverable from HTML — it lives in the JSON view. The
/// binding is reconstructed with an empty action so the tree's shape (which
/// event fired, and in what order) survives, and the client fills the ops in
/// from the view it already holds.
fn parse_event_annotation(value: &str) -> Option<EventBinding> {
    let (event, index) = value.split_once('[')?;
    // Reject a non-numeric suffix so a hand-written `data-velo-on="click"` is
    // still understood rather than discarded as malformed.
    let index = index.trim_end_matches(']');
    if !index.is_empty() && index.parse::<usize>().is_err() {
        return None;
    }
    let event = unescape(event);
    if event.is_empty() {
        return None;
    }
    Some(EventBinding {
        event,
        action: EventAction::new(vec![]),
    })
}

/// Collect every distinct tag name in a tree, in first-visit order.
///
/// Useful after parsing a Maud fragment to assert "this view stopped emitting
/// `<div>` wrappers" or to diff two versions of a template.
pub fn collect_tags(node: &Node, out: &mut Vec<String>) {
    match node {
        Node::Element(el) => {
            if !out.iter().any(|t| t == &el.tag) {
                out.push(el.tag.clone());
            }
            for c in &el.children {
                collect_tags(c, out);
            }
        }
        Node::Fragment(children) | Node::Component { children, .. } => {
            for c in children {
                collect_tags(c, out);
            }
        }
        Node::If { then, otherwise, .. } => {
            collect_tags(then, out);
            collect_tags(otherwise, out);
        }
        Node::Show {
            children, fallback, ..
        } => {
            collect_tags(children, out);
            collect_tags(fallback, out);
        }
        Node::Each { body, .. } => collect_tags(body, out),
        _ => {}
    }
}

/// Every `data-*` attribute value in the tree, keyed by `id attr` -> values.
/// A blunt instrument for tests that assert a payload made it into the markup.
pub fn collect_data_attrs(node: &Node, out: &mut BTreeMap<String, Vec<String>>) {
    match node {
        Node::Element(el) => {
            for a in &el.attrs {
                if let Some(rest) = a.name.strip_prefix("data-") {
                    if let crate::attr::AttrValue::Static(e) = &a.value {
                        if let Some(lit) = e.literal() {
                            out.entry(rest.to_string()).or_default().push(lit);
                        }
                    }
                }
            }
            for c in &el.children {
                collect_data_attrs(c, out);
            }
        }
        Node::Fragment(children) | Node::Component { children, .. } => {
            for c in children {
                collect_data_attrs(c, out);
            }
        }
        Node::If { then, otherwise, .. } => {
            collect_data_attrs(then, out);
            collect_data_attrs(otherwise, out);
        }
        Node::Show {
            children, fallback, ..
        } => {
            collect_data_attrs(children, out);
            collect_data_attrs(fallback, out);
        }
        Node::Each { body, .. } => collect_data_attrs(body, out),
        _ => {}
    }
}

/// Flatten a tree into a `View` with every text run exposed as a payload entry
/// keyed by its path — a debugging aid when comparing a Maud render against a
/// `vhtml!` render.
pub fn text_index(node: &Node, path: &str, out: &mut BTreeMap<String, String>) {
    match node {
        Node::Text(e) => {
            if let Some(lit) = e.literal() {
                out.insert(path.to_string(), lit);
            }
        }
        Node::Fragment(children) => {
            for (i, c) in children.iter().enumerate() {
                text_index(c, &format!("{path}/{i}"), out);
            }
        }
        Node::Element(el) => {
            for (i, c) in el.children.iter().enumerate() {
                text_index(c, &format!("{path}/{}.{}", el.tag, i), out);
            }
        }
        _ => {}
    }
}

/// The set of signals a parsed tree depends on. Always empty for parsed markup —
/// the method exists so callers do not have to special-case the parser's output.
pub fn parsed_signal_decls() -> Vec<crate::view::SignalDecl> {
    Vec::new()
}

/// The parsed tree as a `Value`, for embedding one render inside another.
pub fn to_value(node: &Node) -> Value {
    match node {
        Node::Text(e) => e.eval(&crate::expr::NullResolver),
        Node::Missing => Value::Null,
        _ => Value::Str(crate::html::render_node(node)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html;

    /// Maud emits a plain string; parse -> render must reproduce it.
    fn round_trip(input: &str) -> String {
        let node = parse_with(input, &ParseOptions::new().raw_annotations()).unwrap();
        html::render_node(&node)
    }

    #[test]
    fn simple_element_round_trips() {
        assert_eq!(
            round_trip(r#"<p class="hi">Ada</p>"#),
            r#"<p class="hi">Ada</p>"#
        );
    }

    #[test]
    fn void_elements_round_trip() {
        assert_eq!(round_trip("<br>"), "<br/>");
        assert_eq!(round_trip(r#"<img src="a.png" alt="x">"#), r#"<img src="a.png" alt="x"/>"#);
    }

    #[test]
    fn boolean_attributes_are_recognised() {
        let node = parse(r#"<input disabled type="checkbox">"#).unwrap();
        let Node::Fragment(children) = &node else { panic!() };
        let Node::Element(el) = &children[0] else { panic!() };
        let disabled = el
            .attrs
            .iter()
            .find(|a| a.name == "disabled")
            .expect("disabled attribute");
        assert!(matches!(
            disabled.value,
            crate::attr::AttrValue::Boolean(crate::expr::Expr::Bool(true))
        ));
        assert_eq!(html::render_node(&node), r#"<input disabled type="checkbox"/>"#);
    }

    #[test]
    fn entities_are_decoded_then_re_encoded() {
        assert_eq!(
            round_trip("<span title=\"a &amp; b\">5 &lt; 6</span>"),
            r#"<span title="a &amp; b">5 &lt; 6</span>"#
        );
    }

    #[test]
    fn numeric_entities_decode() {
        assert_eq!(unescape("&#65;&#x42;"), "AB");
        assert_eq!(unescape("&nope;"), "&nope;");
    }

    #[test]
    fn unclosed_p_is_implicitly_closed() {
        let html_in = "<div><p>one<p>two</div>";
        assert_eq!(round_trip(html_in), "<div><p>one</p><p>two</p></div>");
    }

    #[test]
    fn unclosed_list_items() {
        assert_eq!(
            round_trip("<ul><li>a<li>b</ul>"),
            "<ul><li>a</li><li>b</li></ul>"
        );
    }

    #[test]
    fn script_body_is_not_parsed_as_markup() {
        let node = parse(r#"<script>if (a < b) { x(); }</script>"#).unwrap();
        assert_eq!(
            html::render_node(&node),
            r#"<script>if (a < b) { x(); }</script>"#
        );
    }

    #[test]
    fn json_island_becomes_text() {
        let node = parse(r#"<script type="application/json">{"a":1}</script>"#).unwrap();
        let Node::Fragment(children) = &node else { panic!() };
        assert!(matches!(children[0], Node::Text(_)));
        assert_eq!(html::render_node(&node), r#"{"a":1}"#);
    }

    #[test]
    fn event_annotations_become_bindings() {
        let node = parse(r#"<button data-velo-on="click[0]">go</button>"#).unwrap();
        let Node::Fragment(children) = &node else { panic!() };
        let Node::Element(el) = &children[0] else { panic!() };
        assert_eq!(el.events.len(), 1);
        assert_eq!(el.events[0].event, "click");
        assert!(el.events[0].action.ops.is_empty());
        // The annotation attribute itself must not survive as a plain attribute.
        assert!(!el.attrs.iter().any(|a| a.name == EVENT_ANNOTATION));
    }

    #[test]
    fn comments_are_preserved_unless_stripped() {
        assert_eq!(round_trip("<div><!-- hi --></div>"), "<div><!-- hi --></div>");
        let opts = ParseOptions {
            strip_comments: true,
            ..ParseOptions::new()
        };
        let node = parse_with("<div><!-- hi --></div>", &opts).unwrap();
        assert_eq!(html::render_node(&node), "<div></div>");
    }

    #[test]
    fn gritshield_style_table_round_trips() {
        let input = r##"<tr class="divide-x"><td class="p-3"><a href="/admin/users/1" hx-get="/admin/users/1" hx-target="#main-content">Ada</a></td><td><input type="text" value="ada@example.com" name="email"/></td></tr>"##;
        assert_eq!(round_trip(input), input);
    }

    #[test]
    fn multiple_roots_are_allowed_by_default() {
        let node = parse("<b>a</b><i>b</i>").unwrap();
        let Node::Fragment(children) = node else {
            panic!("expected a fragment")
        };
        assert_eq!(children.len(), 2);
    }

    #[test]
    fn single_root_mode_rejects_siblings() {
        let opts = ParseOptions {
            allow_multiple_roots: false,
            ..ParseOptions::new()
        };
        assert!(parse_with("<b>a</b><i>b</i>", &opts).is_err());
        assert!(parse_with("<b>a</b>", &opts).is_ok());
    }

    #[test]
    fn malformed_input_degrades_instead_of_failing() {
        // Unterminated attribute value, stray `<`, unclosed tags: all must parse.
        let _ = parse(r#"<div class="oops><span>"#).unwrap();
        let _ = parse("<<>>").unwrap();
        let _ = parse("<div").unwrap();
    }

    #[test]
    fn tag_index_and_text_index_helpers_work() {
        let node = parse(r#"<div><span>x</span><b>y</b></div>"#).unwrap();
        let mut tags = Vec::new();
        collect_tags(&node, &mut tags);
        assert_eq!(tags, vec!["div", "span", "b"]);

        let mut texts = BTreeMap::new();
        // Path scheme: a fragment names children by index, an element by
        // `tag.index`. So `root/0/div.0/span.0` is the text inside the <span>
        // that is div's first child, which is the fragment's first child.
        text_index(&node, "root", &mut texts);
        assert_eq!(
            texts,
            BTreeMap::from([
                ("root/0/div.0/span.0".to_string(), "x".to_string()),
                ("root/0/div.1/b.0".to_string(), "y".to_string()),
            ])
        );
    }
}
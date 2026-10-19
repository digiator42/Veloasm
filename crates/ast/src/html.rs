//! Server-side rendering: [`View`] -> HTML string.
//!
//! This is the backend half. Given the *same* view a browser will later mount,
//! it produces the first paint as static markup — which is what makes the
//! "write the feature once on the backend" story work in practice: the server
//! already knows the data, so it should not ship an empty shell and re-fetch
//! what it held.
//!
//! The output is **not** the client's DOM. It is the client's *initial state*.
//! The client mounts the AST on top of it (or replaces it wholesale); it does
//! not diff against it. That keeps this renderer small and total — no
//! hydration markers, no mismatch recovery — at the cost of not saving the
//! first-frame round trip. Add hydration markers later without changing the AST
//! if that trade needs revisiting.
//!
//! ## Escaping
//!
//! Text nodes and attribute values are escaped via [`escape_text`] and
//! [`escape_attr`]. The only unescaped content is [`Node::Raw`], which exists
//! precisely so a backend can inject markup it produced itself. A backend that
//! interpolates untrusted data into `Node::Raw` is doing so on purpose; nothing
//! in this crate makes that safe by accident.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::attr::{AttrValue, is_boolean_attribute, is_raw_text_element, is_void_element};
use crate::expr::{Expr, Resolver};
use crate::path::ObjPath;
use crate::node::Node;
use crate::value::Value;
use crate::view::{HandlerRef, SignalDecl, View};

/// Escape a string for insertion into an HTML text node.
pub fn escape_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Escape a string for insertion into a double-quoted HTML attribute value.
///
/// Also escapes `'`, so the result is safe in a single-quoted attribute too.
pub fn escape_attr(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Escape a string for insertion into a `<script>` or `<style>` body.
///
/// Only the sequence that could *terminate* the element is escaped, because
/// that is the only way markup inside `<script>` becomes markup.
pub fn escape_raw_text(input: &str) -> String {
    input.replace("</script", "<\\/script").replace("</style", "<\\/style")
}

/// Escape JSON so it is safe to embed verbatim inside a `<script>` element.
///
/// `escape_text` is wrong here: a `<script>` body is raw text, so an HTML
/// entity would land in the JSON as literal `&lt;` rather than `<`. The
/// characters are therefore emitted as `\uXXXX` escapes, which JSON decodes
/// back to the original and which cannot close the element.
pub fn json_for_script(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for ch in json.chars() {
        match ch {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(ch),
        }
    }
    out
}

/// SSR options.
#[derive(Debug, Clone)]
pub struct HtmlOptions {
    /// Values backing every declared signal at render time.
    ///
    /// Signals are seeded from their declarations by default; supplying a
    /// runtime map lets a backend override a few of them (e.g. after a write)
    /// without rebuilding the view.
    pub signals: BTreeMap<String, Value>,
    /// Route parameters available to [`Expr::RouteParam`].
    pub params: BTreeMap<String, String>,
    /// Query parameters available to [`Expr::Query`].
    pub query: BTreeMap<String, String>,
    /// Value of [`Expr::Route`]. Defaults to empty.
    pub path: String,
    /// Render event bindings as inert `data-velo-on-*` attributes.
    ///
    /// On by default: without it, SSR output silently loses every interactive
    /// affordance and there is no sign in the markup that something is missing.
    /// The client replaces these attributes with real listeners when it mounts.
    pub annotate_events: bool,
    /// Render `data-velo-signal-*` seed attributes for declared signals.
    ///
    /// On by default, so the SSR markup is self-describing and a client can
    /// adopt it without a second fetch.
    pub annotate_signals: bool,
    /// Class added to the root element, so a stylesheet can target
    /// "still server-rendered" and a script can spot it.
    pub root_class: Option<String>,
}

impl Default for HtmlOptions {
    fn default() -> Self {
        HtmlOptions {
            signals: BTreeMap::new(),
            params: BTreeMap::new(),
            query: BTreeMap::new(),
            path: String::new(),
            annotate_events: true,
            annotate_signals: true,
            root_class: None,
        }
    }
}

impl HtmlOptions {
    /// Empty options: signals seeded from their declarations.
    pub fn new() -> Self {
        Self::default()
    }

    /// Override one signal's render-time value.
    pub fn with_signal<K: Into<String>, V: Into<Value>>(mut self, id: K, value: V) -> Self {
        self.signals.insert(id.into(), value.into());
        self
    }

    /// Add a route parameter.
    pub fn with_param<K: Into<String>, V: Into<String>>(mut self, name: K, value: V) -> Self {
        self.params.insert(name.into(), value.into());
        self
    }

    /// Add a query parameter.
    pub fn with_query<K: Into<String>, V: Into<String>>(mut self, key: K, value: V) -> Self {
        self.query.insert(key.into(), value.into());
        self
    }

    /// Set the current path.
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// Add several query parameters at once.
    pub fn with_query_params<K, V, I>(mut self, params: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        for (k, v) in params {
            self.query.insert(k.into(), v.into());
        }
        self
    }

    /// Turn off the inert `data-velo-on-*` / `data-velo-signal-*` annotations.
    pub fn bare(mut self) -> Self {
        self.annotate_events = false;
        self.annotate_signals = false;
        self
    }
}

/// The resolver SSR evaluates expressions against.
struct HtmlResolver<'a> {
    view: &'a View,
    options: &'a HtmlOptions,
    /// Loop bindings, innermost last. Index `0` of each entry is the depth-`0`
    /// (innermost) loop, matching [`Expr::LoopItem`]'s depth convention.
    loops: Vec<(Value, usize)>,
}

impl<'a> HtmlResolver<'a> {
    fn new(view: &'a View, options: &'a HtmlOptions) -> Self {
        HtmlResolver {
            view,
            options,
            loops: Vec::new(),
        }
    }

    fn signal_value(&self, id: &str) -> Value {
        if let Some(v) = self.options.signals.get(id) {
            return v.clone();
        }
        self.view
            .signal(id)
            .map(|s: &SignalDecl| s.initial.clone())
            .unwrap_or(Value::Null)
    }
}

impl Resolver for HtmlResolver<'_> {
    fn signal(&self, id: &str) -> Option<Value> {
        Some(self.signal_value(id))
    }
    fn payload(&self, path: &ObjPath) -> Option<Value> {
        let root = Value::Map(self.view.payload.clone());
        if path.is_root() {
            return Some(root);
        }
        root.path(path)
    }
    fn route_param(&self, name: &str) -> Option<String> {
        self.options.params.get(name).cloned()
    }
    fn query(&self, name: &str) -> Option<String> {
        self.options.query.get(name).cloned()
    }
    fn route(&self) -> String {
        self.options.path.clone()
    }
    fn loop_item(&self, depth: u16) -> Option<Value> {
        self.loops
            .len()
            .checked_sub(depth as usize + 1)
            .and_then(|i| self.loops.get(i))
            .map(|(v, _)| v.clone())
    }
    fn loop_index(&self, depth: u16) -> Option<usize> {
        self.loops
            .len()
            .checked_sub(depth as usize + 1)
            .and_then(|i| self.loops.get(i))
            .map(|(_, idx)| *idx)
    }
}

/// Render a view to an HTML fragment.
///
/// This is the entry point a backend calls: build the [`View`], hand it over,
/// embed the result in a page (or an HTMX partial).
pub fn render(view: &View) -> String {
    render_with(view, &HtmlOptions::default())
}

/// Render a view to an HTML fragment with explicit runtime state.
///
/// The result is exactly the view's markup — no extra elements. That matters:
/// the fragment has to be legal inside whatever the caller pastes it into (a
/// `<tbody>`, an HTMX swap target, a `<head>`). Use [`render_document`] when you
/// want the seed island appended alongside it.
pub fn render_with(view: &View, options: &HtmlOptions) -> String {
    let mut resolver = HtmlResolver::new(view, options);
    let mut out = String::new();
    write_node(&mut out, &view.root, &mut resolver, view, options);
    out
}

/// Render a view plus its [`SeedIsland`].
///
/// Use this when SSR'd markup and the reactive client are both in play and the
/// client must not refetch the seeds. The island is a sibling of the fragment,
/// never nested inside it.
pub fn render_document(view: &View, options: &HtmlOptions) -> String {
    let mut out = render_with(view, options);
    out.push_str(&render_seed_island(view, options));
    out
}

/// Render only the seed island — a `<script type="application/json">` element
/// carrying every signal's value at render time plus the request's route and
/// query state.
///
/// Emitted separately from the fragment so the markup stays position-agnostic:
/// a `<tr>`-rooted view can be embedded in a table without the island dragging
/// the row's parent into an invalid nesting.
pub fn render_seed_island(view: &View, options: &HtmlOptions) -> String {
    let seeds = SeedIsland::from_view(view, options);
    let json = seeds.to_json_string();
    format!(
        r#"<script type="application/json" id="velo-view-seeds" data-ast-version="{}">{}</script>"#,
        view.version,
        json_for_script(&json)
    )
}

/// The declarative seed island emitted next to SSR'd markup.
///
/// Separate from [`View`] on purpose: it carries *runtime* values (route
/// params, the current path) that are a property of the request, not of the
/// view, and it must not be able to smuggle a non-portable node into the client.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SeedIsland {
    /// Every declared signal's value at render time, by id.
    pub signals: BTreeMap<String, Value>,
    /// The path the page was served for.
    pub path: String,
    /// Route parameters active for this request.
    pub params: BTreeMap<String, String>,
    /// Query parameters active for this request.
    pub query: BTreeMap<String, String>,
}

impl SeedIsland {
    /// Collect the seeds for `view` under `options`.
    pub fn from_view(view: &View, options: &HtmlOptions) -> Self {
        let mut signals = BTreeMap::new();
        for decl in &view.signals {
            let v = options
                .signals
                .get(&decl.id)
                .cloned()
                .unwrap_or_else(|| decl.initial.clone());
            signals.insert(decl.id.clone(), v);
        }
        SeedIsland {
            signals,
            path: options.path.clone(),
            params: options.params.clone(),
            query: options.query.clone(),
        }
    }
}

#[cfg(feature = "serde")]
impl SeedIsland {
    /// Serialize the island to JSON.
    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Parse an island from JSON.
    pub fn from_json(s: &str) -> Option<SeedIsland> {
        serde_json::from_str(s).ok()
    }
}

#[cfg(not(feature = "serde"))]
impl SeedIsland {
    fn to_json_string(&self) -> String {
        // Without `serde` there is no way to emit a faithful island; emit an
        // empty object rather than a wrong one. `render_with` is for the server
        // side, where the feature is on.
        "{}".to_string()
    }
}

fn write_node(
    out: &mut String,
    node: &Node,
    r: &mut HtmlResolver,
    view: &View,
    opts: &HtmlOptions,
) {
    match node {
        Node::Missing => {}

        Node::Raw(html) => {
            // The one unescaped path, by design.
            out.push_str(html);
        }

        Node::Text(expr) => {
            let value = expr.eval(r);
            if matches!(value, Value::Null) && matches!(expr, Expr::Missing | Expr::Null) {
                return;
            }
            push_escaped(out, &value.to_text(), false);
        }

        Node::Fragment(children) => {
            for child in children {
                write_node(out, child, r, view, opts);
            }
        }

        Node::If {
            test,
            then,
            otherwise,
        } => {
            if test.eval(r).truthy() {
                write_node(out, then, r, view, opts);
            } else {
                write_node(out, otherwise, r, view, opts);
            }
        }

        Node::Show {
            when,
            children,
            fallback,
        } => {
            // SSR collapses the reactive switch to the branch that is active
            // right now. The client rebuilds both branches and starts swapping.
            if when.eval(r).truthy() {
                write_node(out, children, r, view, opts);
            } else {
                write_node(out, fallback, r, view, opts);
            }
        }

        Node::Each { items, key, body } => {
            let list = items.eval(r);
            let Some(entries) = list.as_list() else {
                // Not a list: render nothing rather than emitting `[object]` or
                // panicking. A backend that meant to pass a list and passed a
                // scalar should see an empty region, not a broken one.
                return;
            };
            for (i, item) in entries.iter().enumerate() {
                r.loops.push((item.clone(), i));
                // `key` participates in the client's reconciliation but has no
                // SSR output of its own.
                let _ = key;
                write_node(out, body, r, view, opts);
                r.loops.pop();
            }
        }

        Node::Component {
            name,
            attrs,
            children,
        } => {
            // Components are client-only by definition: their props are Rust,
            // not HTML. SSR renders a placeholder that carries enough
            // information for the client to take over.
            let _ = attrs;
            let _ = children;
            let _ = write!(
                out,
                r#"<velo-component data-name="{}"></velo-component>"#,
                escape_attr(name)
            );
        }

Node::Slot { name } => {
            // A slot outside a component has nothing to fill it; render nothing.
            let _ = name;
        }

        Node::Native(_) => {
            // A native closure is client-only Rust. It is deliberately not
            // called here: it was written against a browser resolver and may
            // touch the DOM. Rendering it server-side would produce a first
            // paint that disagrees with the client's, which is worse than
            // rendering nothing and having `View::is_portable()` catch it at
            // the boundary where the view is handed to a client.
        }

        Node::Element(el) => {
            let tag = el.tag.as_str();
            let raw_text = is_raw_text_element(tag);
            let _ = write!(out, "<{tag}");

            write_attributes(out, el, r, view, opts, tag);

            if is_void_element(tag) {
                out.push_str("/>");
                return;
            }
            out.push('>');

            if raw_text {
                // Children of <script>/<style> are emitted without escaping
                // except for the terminating sequence.
                for child in &el.children {
                    if let Node::Text(expr) = child {
                        push_escaped(out, &expr.eval(r).to_text(), true);
                    } else {
                        // A structured child inside a raw-text element cannot
                        // be represented as raw text; fall back to its own
                        // rendering.
                        write_node(out, child, r, view, opts);
                    }
                }
            } else {
                for child in &el.children {
                    write_node(out, child, r, view, opts);
                }
            }

            let _ = write!(out, "</{tag}>");
        }
    }
}

fn push_escaped(out: &mut String, text: &str, raw_text: bool) {
    if raw_text {
        out.push_str(&escape_raw_text(text));
    } else {
        out.push_str(&escape_text(text));
    }
}

fn write_attributes(
    out: &mut String,
    el: &crate::node::Element,
    r: &mut HtmlResolver,
    view: &View,
    opts: &HtmlOptions,
    tag: &str,
) {
    // Class is assembled once from every contributor (a base plus any toggles),
    // so a toggle can never clobber the base the way two independent writers
    // would.
    let mut class_parts: Vec<String> = Vec::new();
    let mut class_emitted = false;

    for attr in &el.attrs {
        match &attr.value {
            AttrValue::Ref(name) => {
                let _ = write!(out, r#" data-velo-ref="{}""#, escape_attr(name));
            }

            AttrValue::ClassToggle { class, when } => {
                if when.eval(r).truthy() {
                    class_parts.push(class.clone());
                }
            }

            AttrValue::StyleProp { prop, value } => {
                let v = value.eval(r);
                if v.truthy() {
                    let _ = write!(
                        out,
                        r#" style="{prop}:{}""#,
                        escape_attr(&v.to_text())
                    );
                }
            }

            AttrValue::Boolean(expr) => {
                let v = expr.eval(r);
                if v.truthy() {
                    let _ = write!(out, r#" {}"#, attr.name);
                }
            }

            AttrValue::BindValue(expr) | AttrValue::BindChecked(expr) => {
                // A two-way binding has no meaningful SSR form beyond its
                // current value; emit it as the value it holds so the first
                // paint is correct.
                let v = expr.eval(r);
                let name = if attr.name == "bind:checked" {
                    "checked"
                } else {
                    "value"
                };
                if v.truthy() {
                    let _ = write!(out, r#" {name}="{}""#, escape_attr(&v.to_text()));
                }
            }

            AttrValue::Static(expr) | AttrValue::Bound(expr) => {
                let name = attr.name.as_str();
                if name == "class" {
                    class_parts.extend(expr.eval(r).to_text().split_whitespace().map(String::from));
                    class_emitted = true;
                    continue;
                }
                let value = expr.eval(r);
                if matches!(value, Value::Null) && is_boolean_attribute(name) {
                    continue;
                }
                if is_boolean_attribute(name) {
                    if value.truthy() {
                        let _ = write!(out, r#" {name}"#);
                    }
                    continue;
                }
                if matches!(value, Value::Null) {
                    // A null-bound attribute is omitted, not written empty —
                    // `title=""` and no `title` are different things to a
                    // screen reader.
                    continue;
                }
                let _ = write!(out, r#" {name}="{}""#, escape_attr(&value.to_text()));
            }
        }
    }

    if !class_parts.is_empty() {
        let _ = write!(out, r#" class="{}""#, escape_attr(&class_parts.join(" ")));
    } else if class_emitted {
        // Every class contributor evaluated falsey: still emit the attribute so
        // the client can find it and own the writer.
        out.push_str(r#" class="""#);
    }

    if opts.annotate_events {
        for (i, binding) in el.events.iter().enumerate() {
            let _ = write!(
                out,
                r#" data-velo-on="{}[{}]""#,
                escape_attr(&binding.event),
                i
            );
            if binding.action.needs_network(view) {
                out.push_str(r#" data-velo-net="1""#);
            }
        }
    }

    if let Some(cls) = &opts.root_class {
        if !class_parts.is_empty() {
            let _ = write!(out, r#" data-root-class="{}""#, escape_attr(cls));
        } else {
            let _ = write!(out, r#" class="{}""#, escape_attr(cls));
        }
    }

    let _ = tag;
}

/// Render a `<script type="application/json">` manifest of the view's
/// server-backed handlers: handler id -> route + destination signal.
///
/// Lets a client handler registry learn where remote actions live without
/// receiving the whole [`View`]. Emits an empty string when there are none, so
/// a caller can append it unconditionally.
pub fn render_handler_manifest(handlers: &[HandlerRef]) -> String {
    #[cfg(feature = "serde")]
    {
        let remote: Vec<&HandlerRef> = handlers.iter().filter(|h| h.is_remote()).collect();
        if remote.is_empty() {
            return String::new();
        }
        let json = serde_json::to_string(&remote).unwrap_or_else(|_| "[]".to_string());
        return format!(
            r#"<script type="application/json" id="velo-handlers">{}</script>"#,
            json_for_script(&json)
        );
    }
    #[cfg(not(feature = "serde"))]
    {
        let _ = handlers;
        String::new()
    }
}

/// Convenience for the common GritShield case: SSR a view into an HTMX partial
/// (`hx-get` responses), which is exactly how the admin dashboard already swaps
/// regions today.
pub fn render_partial(view: &View, root_class: &str) -> String {
    render_with(
        view,
        &HtmlOptions {
            root_class: Some(root_class.to_string()),
            annotate_events: true,
            ..HtmlOptions::default()
        },
    )
}

/// Helper for one-line SSR of a bare node with no signals — the "render this
/// small fragment into an email / log line" case.
pub fn render_node(node: &Node) -> String {
    let view = View::new("inline", node.clone());
    render_with(&view, &HtmlOptions::new().bare())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attr::Attr;
    use crate::event::{EventAction, EventOp, EventValue};

    /// SSR with annotations off, so assertions see only the view's own markup.
    fn ssr(view: &View) -> String {
        render_with(view, &HtmlOptions::new().bare())
    }

    #[test]
    fn escapes_text_and_attrs() {
        let view = View::new(
            "t",
            Node::element("p")
                .static_attr("title", "a\"b<c&d")
                .child(Node::text("<script>alert(1)</script>")),
        );
        assert_eq!(
            ssr(&view),
            r#"<p title="a&quot;b&lt;c&amp;d">&lt;script&gt;alert(1)&lt;/script&gt;</p>"#
        );
    }

    #[test]
    fn raw_text_element_children_are_not_html_escaped() {
        let view = View::new(
            "t",
            Node::element("script").child(Node::text("if (a < b) {}")),
        );
        assert_eq!(ssr(&view), "<script>if (a < b) {}</script>");
    }

    #[test]
    fn raw_text_element_cannot_be_closed_from_inside() {
        let view = View::new(
            "t",
            Node::element("script").child(Node::text("x = \"</script><img>\"; y")),
        );
        let html = ssr(&view);
        assert!(!html.contains("</script><img>"), "{html}");
        assert!(html.contains("<\\/script"), "{html}");
    }

    #[test]
    fn void_elements_never_get_a_closing_tag() {
        let view = View::new("t", Node::element("br"));
        assert_eq!(ssr(&view), "<br/>");
    }

    #[test]
    fn boolean_attribute_false_is_omitted() {
        let off = View::new(
            "t",
            Node::element("input").attr(Attr::boolean("disabled", Expr::Bool(false))),
        );
        assert_eq!(ssr(&off), "<input/>");
        let on = View::new(
            "t",
            Node::element("input").attr(Attr::boolean("disabled", Expr::Bool(true))),
        );
        assert_eq!(ssr(&on), "<input disabled/>");
    }

    #[test]
    fn null_bound_attribute_is_omitted_not_emptied() {
        let view = View::new(
            "t",
            Node::element("p").attr(Attr::bound("title", Expr::Null)),
        );
        assert_eq!(ssr(&view), "<p></p>");
    }

    #[test]
    fn class_base_and_toggle_coexist() {
        let view = View::new(
            "t",
            Node::element("div")
                .static_attr("class", "row danger")
                .attr(Attr::class_toggle("selected", Expr::Bool(true)))
                .attr(Attr::class_toggle("hidden", Expr::Bool(false))),
        );
        assert_eq!(ssr(&view), r#"<div class="row danger selected"></div>"#);
    }

    #[test]
    fn style_props_become_a_single_style_attribute() {
        let view = View::new(
            "t",
            Node::element("div")
                .attr(Attr::style_prop("color", Expr::str("red")))
                .attr(Attr::style_prop("display", Expr::Null)),
        );
        assert_eq!(ssr(&view), r#"<div style="color:red"></div>"#);
    }

    #[test]
    fn loop_item_resolves_per_row() {
        let tree = Node::element("ul").child(Node::each(
            Expr::Payload(ObjPath::key("rows")),
            Expr::item_field(0, "id"),
            Node::element("li").child(Node::text_expr(Expr::item_field(0, "name"))),
        ));
        let view = View::new("t", tree).with_payload(
            "rows",
            Value::List(vec![
                Value::map([("id", Value::Num(1.0)), ("name", Value::Str("Ada".into()))]),
                Value::map([("id", Value::Num(2.0)), ("name", Value::Str("Bob".into()))]),
            ]),
        );
        assert_eq!(ssr(&view), "<ul><li>Ada</li><li>Bob</li></ul>");
    }

    #[test]
    fn nested_loops_use_depth_indexing() {
        let tree = Node::each(
            Expr::Payload(ObjPath::key("rows")),
            Expr::item_field(0, "name"),
            Node::fragment(vec![
                Node::text_expr(Expr::item_field(0, "name")),
                Node::text(":"),
                Node::each(
                    Expr::item_field(0, "tags"),
                    Expr::item(0),
                    Node::text_expr(Expr::item(0)),
                ),
            ]),
        );
        let view = View::new("t", tree).with_payload(
            "rows",
            Value::List(vec![Value::map([
                ("name", Value::Str("A".into())),
                (
                    "tags",
                    Value::List(vec![Value::Str("x".into()), Value::Str("y".into())]),
                ),
            ])]),
        );
        assert_eq!(ssr(&view), "A:xy");
    }

    #[test]
    fn reactive_show_collapses_to_the_active_branch() {
        let view = View::new(
            "t",
            Node::show(Expr::signal("ok"), Node::text("yes"), Node::text("no")),
        )
        .with_signal(SignalDecl::new("ok", crate::view::SignalKind::Scalar));
        assert_eq!(ssr(&view), "no");

        let opts = HtmlOptions::new().bare().with_signal("ok", true);
        assert_eq!(render_with(&view, &opts), "yes");
    }

    #[test]
    fn each_over_a_non_list_renders_nothing() {
        let tree = Node::element("ul").child(Node::each(
            Expr::str("not a list"),
            Expr::item(0),
            Node::element("li"),
        ));
        assert_eq!(ssr(&View::new("t", tree)), "<ul></ul>");
    }

    #[test]
    fn event_bindings_are_annotated_not_executed() {
        let view = View::new(
            "t",
            Node::element("button").on(
                "click",
                EventAction::one(EventOp::Set {
                    target: "count".into(),
                    value: EventValue::Expr(Expr::Num(1.0)),
                }),
            ),
        );
        let html = render_with(&view, &HtmlOptions::new());
        assert!(html.contains(r#"data-velo-on="click[0]""#), "{html}");
    }

    #[test]
    fn remote_handlers_are_flagged_as_network_backed() {
        let view = View::new(
            "t",
            Node::element("button").on("click", EventAction::one(EventOp::call("disable"))),
        )
        .with_handler(HandlerRef::remote(
            "disable",
            crate::event::ServerRoute::post("/api/rules/42/disable"),
            "rule",
        ));
        let html = render_with(&view, &HtmlOptions::new());
        assert!(html.contains(r#"data-velo-net="1""#), "{html}");
    }

    #[test]
    fn seed_island_is_a_sibling_not_a_child() {
        let view =
            View::new("t", Node::element("tr").child(Node::text("row")))
                .with_signal(SignalDecl::count("count", 3.0));
        let doc = render_document(&view, &HtmlOptions::new());
        assert!(doc.contains("</tr><script"), "{doc}");
        assert!(doc.contains("count"), "{doc}");
    }

    #[test]
    fn json_for_script_neutralises_closing_tags() {
        assert_eq!(
            json_for_script("{\"a\":\"</script>\"}"),
            "{\"a\":\"\\u003c/script\\u003e\"}"
        );
        assert_eq!(json_for_script("a<b&c"), "a\\u003cb\\u0026c");
    }

    #[test]
    fn raw_html_is_not_escaped() {
        let view = View::new("t", Node::raw("<b>bold</b>"));
        assert_eq!(ssr(&view), "<b>bold</b>");
    }

    #[test]
    fn raw_is_reported_as_non_portable() {
        let view = View::new("t", Node::raw("<b>x</b>"));
        assert!(!view.is_portable());
        assert!(!view.portability_errors().is_empty());
    }

    #[test]
    fn component_becomes_a_client_owned_placeholder() {
        let view = View::new(
            "t",
            Node::Component {
                name: "ThreatPanel".into(),
                attrs: vec![],
                children: vec![],
            },
        );
        assert_eq!(
            ssr(&view),
            r#"<velo-component data-name="ThreatPanel"></velo-component>"#
        );
    }
}

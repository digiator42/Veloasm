//! View structure: the tree half of the AST.
//!
//! Where [`Expr`](crate::Expr) describes *what a value is*, [`Node`] describes
//! *what the page is*. The variant set is chosen so that everything the
//! framework's `view!` macro can express — and everything Maud's `html!` can
//! express — has a serializable form with **no loss of reactivity**:
//!
//! | construct | node |
//! |---|---|
//! | `<div class="…">` | [`Node::Element`] |
//! | `<>…</>` | [`Node::Fragment`] |
//! | `{ text }` | [`Node::Text`] |
//! | `@if` | [`Node::If`] (static) / [`Node::Show`] (reactive) |
//! | `@for` | [`Node::Each`] |
//! | `<UserCard …>` | [`Node::Component`] + [`Node::Slot`] |
//! | already-rendered HTML | [`Node::Raw`] |

use crate::attr::Attr;
use crate::event::{EventAction, EventBinding};
use crate::expr::{Expr, SignalId};
use std::rc::Rc;

/// A node in a view tree.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "n", content = "v"))]
pub enum Node {
    /// Renders nothing at all — not even a text node. Used for the absent
    /// branch of a conditional and for optional children.
    Missing,
    /// An element with attributes, event bindings, and children.
    Element(Element),
    /// Sibling nodes with no wrapper. Corresponds to `<>…</>` in `view!` and
    /// to a bare `{ … }` body in Maud.
    Fragment(Vec<Node>),
    /// A text node whose content is an expression. Reactive by construction:
    /// the renderer subscribes it to whatever the expression reads.
    Text(Expr),
    /// Pre-rendered HTML, inserted verbatim.
    ///
    /// The seam that lets unmodified Maud `html!` output participate: a backend
    /// renders a `Markup` fragment, wraps it in [`Node::Raw`], and the client
    /// mounts it as-is. Deliberately **not** escapable and deliberately
    /// non-reactive — only feed it markup you produced yourself.
    Raw(String),
    /// Conditional, chosen once at build time.
    ///
    /// Used when the test is static. A reactive test must use [`Node::Show`] so
    /// the branch can swap without rebuilding the subtree.
    If {
        /// The condition, using [`Value::truthy`](crate::Value::truthy).
        test: Expr,
        /// Taken when truthy.
        then: Box<Node>,
        /// Taken otherwise.
        otherwise: Box<Node>,
    },
    /// Conditional that swaps branches when the test flips, keeping both
    /// subtrees alive so their effects and DOM survive the swap.
    ///
    /// The reactive counterpart of [`Node::If`], and the node the built-in
    /// `Show` / `Suspense` components lower to.
    Show {
        /// The condition.
        when: Expr,
        /// Rendered while `when` is truthy.
        children: Box<Node>,
        /// Rendered while `when` is falsey. [`Node::Missing`] renders nothing.
        fallback: Box<Node>,
    },
    /// Repetition over a list expression.
    ///
    /// With a `key`, the renderer reconciles by identity — moving real DOM nodes
    /// instead of recreating them — which is what keeps a 400-row admin table at
    /// one-time cost per structural change. Without a key it behaves like a
    /// plain index loop.
    Each {
        /// The list to repeat over. Evaluated per effect run.
        items: Expr,
        /// Stable identity per item. When absent, the index is used.
        key: Option<Expr>,
        /// Rendered once per item.
        ///
        /// Inside the body, [`Expr::LoopItem`] / [`Expr::LoopIndex`] address the
        /// current binding, and nested `Each` bodies increase the depth.
        body: Box<Node>,
    },
    /// A named component, resolved by the renderer against a component table.
    ///
    /// Attributes and children become its props. `name` is matched
    /// case-sensitively against the registered set, so backend and client must
    /// agree on the spelling.
    Component {
        /// The registered component name.
        name: String,
        /// Props, by the same rules as element attributes.
        attrs: Vec<Attr>,
        /// Child nodes, exposed to the component through [`Node::Slot`]s.
        children: Vec<Node>,
    },
    /// A placeholder inside a component body for content passed from outside.
    Slot {
        /// The slot name. `children` (or an empty string) is the implicit
        /// default slot.
        name: String,
    },
    /// A client-only node produced by arbitrary Rust, re-evaluated whenever its
    /// dependencies change.
    ///
    /// This is what makes a *Maud-shaped* loop reactive. The closure is called
    /// inside an effect, so a signal read within it subscribes that effect; the
    /// subtree it produces is then swapped in. The client therefore gets a list
    /// that updates in place, from Rust code that looks exactly like the Maud
    /// it replaces:
    ///
    /// ```rust
    /// use velo_ast::Node;
    ///
    /// let node = Node::native_render(|_resolver| {
    ///     let mut rows = Vec::new();
    ///     for t in vec!["a", "b"] {
    ///         rows.push(Node::text(t));
    ///     }
    ///     Node::fragment(rows)
    /// });
    /// ```
    ///
    /// **Client-only and never serializable** — the closure is Rust. A view
    /// that ships to a browser must use [`Node::Each`] instead, which is
    /// data-driven and therefore portable.
    /// deliberately *not* `#[serde(skip)]`: skipping would silently serialize
    /// as `{"n":"Native"}` and hand the client a node it cannot render. The
    /// field's own `Serialize` impl errors instead, so the boundary is loud.
    Native(NativeRenderFn),
}

/// A Rust closure that builds a node.
#[derive(Clone)]
pub struct NativeRenderFn(Rc<dyn Fn(&dyn crate::expr::Resolver) -> Node + 'static>);

impl NativeRenderFn {
    /// Wrap a closure.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&dyn crate::expr::Resolver) -> Node + 'static,
    {
        NativeRenderFn(Rc::new(f))
    }

    /// Call the closure.
    pub fn call(&self, r: &dyn crate::expr::Resolver) -> Node {
        (self.0)(r)
    }
}

impl std::fmt::Debug for NativeRenderFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NativeRenderFn(..)")
    }
}

impl PartialEq for NativeRenderFn {
    /// Identity comparison: two native renderers are equal only if they wrap
    /// the same allocation.
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Default for NativeRenderFn {
    /// Renders nothing. Never constructed deliberately — kept because the
    /// derived `Deserialize` needs the type to be constructible before it can
    /// hand control to this impl, which rejects the input anyway.
    fn default() -> Self {
        NativeRenderFn::new(|_| Node::Missing)
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for NativeRenderFn {
    fn serialize<S: serde::Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom(
            "Node::Native is client-only and cannot be serialized — check \
             View::is_portable() before shipping this view",
        ))
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for NativeRenderFn {
    fn deserialize<D: serde::Deserializer<'de>>(_d: D) -> Result<Self, D::Error> {
        Err(serde::de::Error::custom(
            "Node::Native is client-only and cannot be deserialized",
        ))
    }
}

impl Node {
    /// A text node.
    ///
    /// Accepts anything that renders — `&str`, `String`, a number, a bool, or an
    /// [`Expr`] for reactive content:
    ///
    /// ```rust
    /// use velo_ast::{Expr, Node};
    ///
    /// let a = Node::text("hello");
    /// let b = Node::text(42.0);
    /// let c = Node::text(Expr::signal("count"));
    /// assert!(matches!(a, Node::Text(_)));
    /// ```
    pub fn text<T: Into<Node>>(s: T) -> Node {
        s.into()
    }

    /// A text node driven by an explicit expression. A synonym for
    /// `Node::text(expr)` that reads better when the expression is already an
    /// `Expr`.
    pub fn text_expr(e: Expr) -> Node {
        Node::Text(e)
    }

    /// A wrapper element with no attributes or children.
    ///
    /// Returns the [`Element`] *builder*, not a `Node`, so the chainable
    /// `attr`/`on`/`child` methods are usable at the call site. Convert with
    /// `.into()` where a `Node` is required; every insertion point that takes
    /// child content accepts `impl Into<Node>`, so it is usually unnecessary.
    pub fn element(tag: impl Into<String>) -> Element {
        Element::new(tag)
    }

    /// The empty node — renders nothing, not even a text node.
    pub fn missing() -> Node {
        Node::Missing
    }

    /// Verbatim pre-rendered markup.
    pub fn raw(html: impl Into<String>) -> Node {
        Node::Raw(html.into())
    }

    /// A group of sibling nodes.
    pub fn fragment<I, N>(children: I) -> Node
    where
        I: IntoIterator<Item = N>,
        N: Into<Node>,
    {
        Node::Fragment(children.into_iter().map(Into::into).collect())
    }

    /// A static conditional.
    pub fn cond<T, F>(test: Expr, then: T, otherwise: F) -> Node
    where
        T: Into<Node>,
        F: Into<Node>,
    {
        Node::If {
            test,
            then: Box::new(then.into()),
            otherwise: Box::new(otherwise.into()),
        }
    }

    /// A reactive conditional that swaps branches in place.
    pub fn show<C, F>(when: Expr, children: C, fallback: F) -> Node
    where
        C: Into<Node>,
        F: Into<Node>,
    {
        Node::Show {
            when,
            children: Box::new(children.into()),
            fallback: Box::new(fallback.into()),
        }
    }

    /// A keyed repetition.
    pub fn each<B>(items: Expr, key: Expr, body: B) -> Node
    where
        B: Into<Node>,
    {
        Node::Each {
            items,
            key: Some(key),
            body: Box::new(body.into()),
        }
    }

    /// An unkeyed repetition.
    pub fn each_indexed<B>(items: Expr, body: B) -> Node
    where
        B: Into<Node>,
    {
        Node::Each {
            items,
            key: None,
            body: Box::new(body.into()),
        }
    }

    /// A reactive node produced by arbitrary Rust.
    ///
    /// The closure runs inside an effect: a signal it reads subscribes that
    /// effect, and the subtree it returns is swapped in on every change. This
    /// is how `vhtml!`'s `@for` stays reactive while keeping Maud's plain-Rust
    /// loop body — no `Value` ceremony, no rewrites of your field accesses.
    ///
    /// The tradeoff is portability: a closure cannot be serialized, so a view
    /// containing this will refuse to ship. Use [`Node::each`] for anything
    /// that must reach a client as data.
    ///
    /// ```
    /// use velo_ast::Node;
    ///
    /// let count = Node::native_render(|r| {
    ///     let n = r.signal("count").and_then(|v| v.as_f64()).unwrap_or(0.0);
    ///     Node::fragment((0..n as i64).map(|i| Node::text(format!("row {i}"))))
    /// });
    /// ```
    pub fn native_render<F>(build: F) -> Node
    where
        F: Fn(&dyn crate::expr::Resolver) -> Node + 'static,
    {
        Node::Native(NativeRenderFn::new(build))
    }

    /// `true` when this subtree can be serialized and shipped to a client.
    /// Fails on [`Node::Raw`] (server-only) and [`Node::Native`] (a Rust
    /// closure, client-only) — check before shipping, and the message says
    /// which alternative to reach for.
    pub fn is_portable(&self) -> bool {
        match self {
            Node::Raw(_) | Node::Native(_) => false,
            Node::Missing => true,
            Node::Text(e) => e.is_portable(),
            Node::Fragment(children) => children.iter().all(Node::is_portable),
            Node::Element(el) => el.is_portable(),
            Node::If {
                test,
                then,
                otherwise,
            } => test.is_portable() && then.is_portable() && otherwise.is_portable(),
            Node::Show {
                when,
                children,
                fallback,
            } => when.is_portable() && children.is_portable() && fallback.is_portable(),
            Node::Each { items, key, body } => {
                items.is_portable()
                    && key.as_ref().is_none_or(|k| k.is_portable())
                    && body.is_portable()
            }
            Node::Component {
                attrs, children, ..
            } => {
                attrs
                    .iter()
                    .all(|a| a.expr().is_none_or(|e| e.is_portable()))
                    && children.iter().all(Node::is_portable)
            }
            Node::Slot { .. } => true,
        }
    }

    /// Signals read anywhere in this subtree, in first-visit order.
    pub fn signal_reads(&self, out: &mut Vec<SignalId>) {
        match self {
            Node::Text(e) => e.signal_reads(out),
            Node::Fragment(children) => children.iter().for_each(|c| c.signal_reads(out)),
            Node::Element(el) => {
                for attr in &el.attrs {
                    if let Some(e) = attr.expr() {
                        e.signal_reads(out);
                    }
                }
                for b in &el.events {
                    for op in &b.action.ops {
                        collect_op_reads(op, out);
                    }
                }
                el.children.iter().for_each(|c| c.signal_reads(out));
            }
            Node::If {
                test,
                then,
                otherwise,
            } => {
                test.signal_reads(out);
                then.signal_reads(out);
                otherwise.signal_reads(out);
            }
            Node::Show {
                when,
                children,
                fallback,
            } => {
                when.signal_reads(out);
                children.signal_reads(out);
                fallback.signal_reads(out);
            }
            Node::Each { items, key, body } => {
                items.signal_reads(out);
                if let Some(k) = key {
                    k.signal_reads(out);
                }
                body.signal_reads(out);
            }
            Node::Component {
                attrs, children, ..
            } => {
                for attr in attrs {
                    if let Some(e) = attr.expr() {
                        e.signal_reads(out);
                    }
                }
children.iter().for_each(|c| c.signal_reads(out));
            }
            // A native closure's reads are invisible to a static walk: it only
            // ever runs in a browser, so whatever signals it touches must
            // already be declared by whoever authored it.
            Node::Missing | Node::Raw(_) | Node::Native(_) | Node::Slot { .. } => {}
        }
    }

    /// Signals written by any event binding in this subtree, in first-visit
    /// order. A backend can use this to auto-declare the signals its view needs.
    pub fn write_targets(&self, out: &mut Vec<SignalId>) {
        match self {
            Node::Fragment(children) => children.iter().for_each(|c| c.write_targets(out)),
            Node::Element(el) => {
                for b in &el.events {
                    for target in b.action.write_targets() {
                        if !out.contains(&target) {
                            out.push(target);
                        }
                    }
                }
                el.children.iter().for_each(|c| c.write_targets(out));
            }
            Node::Component {
                children,
                attrs,
                ..
            } => {
                // A two-way binding both reads and writes its signal, so it
                // belongs in the declaration set as well as the write set.
                for attr in attrs {
                    if let crate::attr::AttrValue::BindValue(e)
                    | crate::attr::AttrValue::BindChecked(e) = &attr.value
                    {
                        e.signal_reads(out);
                    }
                }
                children.iter().for_each(|c| c.write_targets(out));
            }
            Node::If { then, otherwise, .. } => {
                then.write_targets(out);
                otherwise.write_targets(out);
            }
            Node::Show {
                children, fallback, ..
            } => {
                children.write_targets(out);
                fallback.write_targets(out);
            }
            Node::Each { body, .. } => body.write_targets(out),
            // Unknowable: the closure's writes are inside Rust code that never
            // runs on the server, so its signals must be declared by hand.
            Node::Missing | Node::Text(_) | Node::Raw(_) | Node::Native(_) | Node::Slot { .. } => {}
        }
    }

    /// Every handler invoked by this subtree, in first-visit order.
    pub fn handler_calls(&self, out: &mut Vec<crate::view::HandlerId>) {
        match self {
            Node::Fragment(children) => children.iter().for_each(|c| c.handler_calls(out)),
            Node::Element(el) => {
                for b in &el.events {
                    for h in b.action.handler_calls() {
                        if !out.contains(&h) {
                            out.push(h);
                        }
                    }
                }
                el.children.iter().for_each(|c| c.handler_calls(out));
            }
            Node::Component { children, .. } => {
                children.iter().for_each(|c| c.handler_calls(out))
            }
            Node::If { then, otherwise, .. } => {
                then.handler_calls(out);
                otherwise.handler_calls(out);
            }
            Node::Show {
                children, fallback, ..
            } => {
                children.handler_calls(out);
                fallback.handler_calls(out);
            }
            Node::Each { body, .. } => body.handler_calls(out),
            // A native closure may call handlers, but the call site is inside
            // Rust that the server never executes. `vhtml!` binds local
            // closures directly and needs no handler table.
            Node::Missing | Node::Text(_) | Node::Raw(_) | Node::Native(_) | Node::Slot { .. } => {}
        }
    }
}

fn collect_op_reads(op: &crate::event::EventOp, out: &mut Vec<SignalId>) {
    use crate::event::{EventOp, EventValue};
    match op {
        EventOp::Set { value, .. } | EventOp::Push { value, .. } => {
            if let EventValue::Expr(e) = value {
                e.signal_reads(out)
            }
        }
        EventOp::Update { by, .. } => {
            if let EventValue::Expr(e) = by {
                e.signal_reads(out)
            }
        }
        EventOp::Navigate { to } => to.signal_reads(out),
        EventOp::Debounce { ops, .. } => ops.iter().for_each(|o| collect_op_reads(o, out)),
        EventOp::Toggle { .. }
        | EventOp::Clear { .. }
        | EventOp::Call { .. }
        | EventOp::AllowDefault => {}
    }
}

/// An HTML element node.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Element {
    /// The tag name. Lowercase for real HTML; the runtime dispatches to the
    /// component table before treating it as an element, so a PascalCase name
    /// here is a component reference rather than an unknown tag.
    pub tag: String,
    /// Attribute bindings, in declaration order.
    pub attrs: Vec<Attr>,
    /// `on:<event>` bindings, in declaration order.
    ///
    /// A list rather than a map because two handlers on one event are legal and
    /// meaningful (`click` → update, then `click` → submit), and because order
    /// determines execution order.
    pub events: Vec<EventBinding>,
    /// Child nodes, in document order.
    pub children: Vec<Node>,
}

impl Element {
    /// A bare element.
    pub fn new(tag: impl Into<String>) -> Self {
        Element {
            tag: tag.into(),
            attrs: Vec::new(),
            events: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Add an attribute, returning `self` for chaining.
    pub fn attr(mut self, attr: Attr) -> Self {
        self.attrs.push(attr);
        self
    }

    /// Add a static attribute, returning `self` for chaining.
    pub fn static_attr(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.attrs.push(Attr::stat(name, value));
        self
    }

    /// Bind an event, returning `self` for chaining.
    pub fn on(mut self, event: impl Into<String>, action: EventAction) -> Self {
        self.events.push(EventBinding::new(event, action));
        self
    }

    /// Append a child, returning `self` for chaining.
    pub fn child<N: Into<Node>>(mut self, node: N) -> Self {
        self.children.push(node.into());
        self
    }

    /// Append children, returning `self` for chaining.
    pub fn children<I, N>(mut self, nodes: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<Node>,
    {
        self.children.extend(nodes.into_iter().map(Into::into));
        self
    }

    /// `true` when this element can be serialized.
    pub fn is_portable(&self) -> bool {
        self.attrs.iter().all(|a| a.expr().is_none_or(|e| e.is_portable()))
            && self
                .events
                .iter()
                .all(|b| b.action.ops.iter().all(event_op_portable))
            && self.children.iter().all(Node::is_portable)
    }
}

/// An [`Element`] is a [`Node`], so a builder chain can be handed straight to
/// anything that takes a node.
impl From<Element> for Node {
    fn from(el: Element) -> Node {
        Node::Element(el)
    }
}

/// An expression is a text node — the common shorthand for `{ value }`.
impl From<Expr> for Node {
    fn from(e: Expr) -> Node {
        Node::Text(e)
    }
}

// Anything with an obvious textual rendering becomes a text node, so a view
// builder never has to call `.to_string()` at a call site just to satisfy
// `Node::text`.
macro_rules! node_from_display {
    ($($t:ty),* $(,)?) => {
        $(
            impl From<$t> for Node {
                fn from(v: $t) -> Node {
                    Node::Text(Expr::Str(v.to_string()))
                }
            }
        )*
    };
}
node_from_display!(String, &str, &String, bool, char, f32, f64, i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize);

/// `Option<T>` becomes a text node when present and nothing at all when absent —
/// so `{ maybe_name }` disappears instead of printing `"None"`.
impl<T: Into<Node>> From<Option<T>> for Node {
    fn from(v: Option<T>) -> Node {
        match v {
            Some(inner) => inner.into(),
            None => Node::Missing,
        }
    }
}

fn event_op_portable(op: &crate::event::EventOp) -> bool {
    use crate::event::{EventOp, EventValue};
    match op {
        EventOp::Set { value, .. } | EventOp::Push { value, .. } => match value {
            EventValue::Expr(e) => e.is_portable(),
            _ => true,
        },
        EventOp::Update { by, .. } => match by {
            EventValue::Expr(e) => e.is_portable(),
            _ => true,
        },
        EventOp::Navigate { to } => to.is_portable(),
        EventOp::Debounce { ops, .. } => ops.iter().all(event_op_portable),
        EventOp::Toggle { .. }
        | EventOp::Clear { .. }
        | EventOp::Call { .. }
        | EventOp::AllowDefault => true,
    }
}
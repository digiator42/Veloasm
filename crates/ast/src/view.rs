//! The root payload: a self-describing, serializable view.
//!
//! A [`View`] is what actually crosses the wire. It carries not just the tree
//! but everything the client needs to bring it alive — the signal declarations
//! (so the browser knows which cells to materialize, and with what seed), the
//! handler table (so an event can name a client closure *or* a backend route),
//! and a static payload for server-computed data.
//!
//! ```rust
//! use velo_ast::{Expr, Node, SignalDecl, SignalKind, View};
//!
//! let view = View::new("threat-panel", Node::text("Loading…"))
//!     .with_signal(SignalDecl::new("threats", SignalKind::List))
//!     .with_root(Node::element("ul").child(Node::each(
//!         Expr::signal("threats"),
//!         Expr::item_field(0, "id"),
//!         Node::element("li").child(Node::text(Expr::item_field(0, "name"))),
//!     )));
//!
//! assert!(view.is_portable());
//! assert_eq!(view.signal("threats").unwrap().kind, SignalKind::List);
//! ```

use std::collections::BTreeMap;

use crate::attr::AttrValue;
pub use crate::event::{HandlerId, ServerRoute};
use crate::expr::SignalId;
use crate::node::Node;
use crate::value::Value;

/// The shape of a signal, used by the client to pick a storage strategy and by
/// [`EventOp::Clear`] to know what "reset" means.
///
/// # Why this matters
///
/// `SignalKind::List` is not decoration: an unkeyed admin grid that appends
/// thousands of log lines needs a capped ring buffer, and the runtime cannot
/// infer that from a value it only sees after the fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum SignalKind {
    /// A single value. The default.
    #[default]
    Scalar,
    /// A list. Event pushes append; [`EventOp::Clear`] empties it.
    List,
    /// A map/object.
    Map,
}

/// One reactive cell the view expects the client to materialize.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SignalDecl {
    /// The id every `Expr::Signal` and `EventOp` in this view refers to.
    pub id: SignalId,
    /// The seed value. A backend that has already computed the initial data
    /// (the common case) puts it here so the client renders real content on the
    /// first frame instead of flashing an empty state.
    pub initial: Value,
    /// The storage shape.
    pub kind: SignalKind,
    /// Soft cap: when a `List` signal exceeds this length, the runtime drops
    /// from the front. `None` means unbounded. This is how a log panel stays
    /// at 200 rows without the view having to think about it.
    pub cap: Option<usize>,
}

impl SignalDecl {
    /// A signal declaration with an empty seed.
    pub fn new(id: impl Into<SignalId>, kind: SignalKind) -> Self {
        SignalDecl {
            id: id.into(),
            initial: match kind {
                SignalKind::Scalar => Value::Null,
                SignalKind::List => Value::List(Vec::new()),
                SignalKind::Map => Value::Map(BTreeMap::new()),
            },
            kind,
            cap: None,
        }
    }

    /// A scalar seeded with `value`.
    pub fn scalar(id: impl Into<SignalId>, value: impl Into<Value>) -> Self {
        SignalDecl {
            id: id.into(),
            initial: value.into(),
            kind: SignalKind::Scalar,
            cap: None,
        }
    }

    /// A map seeded with `value`.
    ///
    /// The shape a server-owned projection arrives in: one signal holding the
    /// whole answer, so a handler that replaces it updates every part of the
    /// page that reads from it rather than only the field it happened to write.
    pub fn map(id: impl Into<SignalId>, value: impl Into<Value>) -> Self {
        SignalDecl {
            id: id.into(),
            initial: value.into(),
            kind: SignalKind::Map,
            cap: None,
        }
    }

    /// A scalar seeded with a number.
    pub fn count(id: impl Into<SignalId>, start: f64) -> Self {
        SignalDecl::scalar(id, Value::Num(start))
    }

    /// A list seeded with `items`.
    pub fn list<I, V>(id: impl Into<SignalId>, items: I) -> Self
    where
        I: IntoIterator<Item = V>,
        V: Into<Value>,
    {
        SignalDecl {
            id: id.into(),
            initial: Value::List(items.into_iter().map(Into::into).collect()),
            kind: SignalKind::List,
            cap: None,
        }
    }

    /// Cap a `List` signal, evicting from the front once it is exceeded.
    pub fn capped(mut self, cap: usize) -> Self {
        self.cap = Some(cap);
        self
    }
}

/// A handler an event can invoke.
///
/// With `server: None` the id resolves against a client-local registry — a
/// closure the wasm app registered. With `server: Some(..)` it resolves to a
/// backend endpoint: the client issues the request, decodes the reply, and
/// writes it into `writes`. That second form is the whole point of this crate —
/// a feature authored once in a Rust backend behaves as a live, reactive UI in
/// the browser without its logic ever being compiled to WASM.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HandlerRef {
    /// The name events use.
    pub id: HandlerId,
    /// The backend endpoint, if this handler lives on the server.
    pub server: Option<ServerRoute>,
    /// The signal the decoded response is written into. Required when
    /// `server` is set, so the client always knows where the reply lands.
    pub writes: Option<SignalId>,
}

impl HandlerRef {
    /// A client-local handler, resolved by name at dispatch time.
    pub fn local(id: impl Into<HandlerId>) -> Self {
        HandlerRef {
            id: id.into(),
            server: None,
            writes: None,
        }
    }

    /// A server-backed handler writing its reply into `signal`.
    pub fn remote(id: impl Into<HandlerId>, route: ServerRoute, signal: impl Into<SignalId>) -> Self {
        HandlerRef {
            id: id.into(),
            server: Some(route),
            writes: Some(signal.into()),
        }
    }

    /// `true` when this handler issues a network request.
    pub fn is_remote(&self) -> bool {
        self.server.is_some()
    }
}

/// A complete, self-describing view: the unit of exchange between a backend and
/// a client.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct View {
    /// The AST revision, checked by [`View::from_json`].
    pub version: u16,
    /// A human-readable name, used in diagnostics and error overlays.
    pub name: String,
    /// The tree.
    pub root: Node,
    /// Signals the client must materialize before mounting `root`.
    pub signals: Vec<SignalDecl>,
    /// Handlers the client must resolve before mounting `root`.
    pub handlers: Vec<HandlerRef>,
    /// Static, non-reactive data available to [`Expr::Payload`] reads. This is
    /// where a backend puts values it already has and that never change — a
    /// user's display name, a CSRF token, a permission list.
    pub payload: BTreeMap<String, Value>,
}

impl View {
    /// An empty view named `name`, rooted at `root`.
    pub fn new<R: Into<Node>>(name: impl Into<String>, root: R) -> Self {
        View {
            version: crate::AST_VERSION,
            name: name.into(),
            root: root.into(),
            signals: Vec::new(),
            handlers: Vec::new(),
            payload: BTreeMap::new(),
        }
    }

    /// Declare a signal (builder style).
    pub fn with_signal(mut self, decl: SignalDecl) -> Self {
        self.signals.push(decl);
        self
    }

    /// Declare a handler (builder style).
    pub fn with_handler(mut self, handler: HandlerRef) -> Self {
        self.handlers.push(handler);
        self
    }

    /// Add static payload (builder style).
    pub fn with_payload<K: Into<String>, V: Into<Value>>(mut self, key: K, value: V) -> Self {
        self.payload.insert(key.into(), value.into());
        self
    }

    /// Replace the root tree (builder style).
    pub fn with_root<R: Into<Node>>(mut self, root: R) -> Self {
        self.root = root.into();
        self
    }

    /// Look up a signal declaration by id.
    pub fn signal(&self, id: &str) -> Option<&SignalDecl> {
        self.signals.iter().find(|s| s.id == id)
    }

    /// Look up a handler by id.
    pub fn handler(&self, id: &str) -> Option<&HandlerRef> {
        self.handlers.iter().find(|h| h.id.as_str() == id)
    }

    /// Look up a payload value by key.
    pub fn payload_value(&self, key: &str) -> Option<&Value> {
        self.payload.get(key)
    }

    /// The initial value for `id`, or `Null`.
    pub fn seed(&self, id: &str) -> Value {
        self.signal(id)
            .map(|s| s.initial.clone())
            .unwrap_or(Value::Null)
    }

    /// Every signal id this view reads but never declares.
    ///
    /// A useful pre-flight check: an undeclared read silently renders empty, so
    /// catching it here turns a blank cell into a build/test failure. The
    /// client's own signal table satisfies the same obligation at runtime.
    pub fn undeclared_reads(&self) -> Vec<SignalId> {
        let mut reads = Vec::new();
        self.root.signal_reads(&mut reads);
        reads
            .into_iter()
            .filter(|id| self.signal(id).is_none())
            .collect()
    }

    /// Every handler this view invokes but never declares.
    pub fn undeclared_handlers(&self) -> Vec<HandlerId> {
        let mut calls = Vec::new();
        self.root.handler_calls(&mut calls);
        calls.into_iter().filter(|h| self.handler(h.as_str()).is_none()).collect()
    }

    /// Declare every signal the tree reads, seeding undeclared ones with the
    /// shape their use implies. Idempotent — existing declarations win.
    ///
    /// Lets a builder write `Expr::signal("count")` without also writing
    /// `SignalDecl::count("count", 0)`.
    pub fn declare_used_signals(mut self) -> Self {
        let mut reads = Vec::new();
        self.root.signal_reads(&mut reads);
        for id in reads {
            if self.signal(&id).is_some() {
                continue;
            }
            self.signals.push(SignalDecl::new(id, SignalKind::Scalar));
        }
        let mut writes = Vec::new();
        self.root.write_targets(&mut writes);
        for id in writes {
            if self.signal(&id).is_some() {
                continue;
            }
            self.signals.push(SignalDecl::new(id, SignalKind::Scalar));
        }
        self
    }

    /// Declare every handler the tree invokes, as client-local placeholders.
    /// A remote handler registered by the backend overrides the placeholder.
    pub fn declare_used_handlers(mut self) -> Self {
        let mut calls = Vec::new();
        self.root.handler_calls(&mut calls);
        for id in calls {
            if self.handler(id.as_str()).is_none() {
                self.handlers.push(HandlerRef::local(id));
            }
        }
        self
    }

    /// `true` when the whole view can be serialized and shipped to a client.
    ///
    /// Fails on [`Node::Raw`] and on any [`Expr::Native`] — both are explicit
    /// "this is not portable" markers, so a backend can assert this instead of
    /// discovering the problem as a JSON error at runtime.
    pub fn is_portable(&self) -> bool {
        self.root.is_portable()
    }

    /// Why the view is not portable, or `None` when it is. Every
    /// non-portable site is listed, not just the first.
    pub fn portability_errors(&self) -> Vec<String> {
        let mut errs = Vec::new();
        collect_portability(&self.root, "root", &mut errs);
        errs
    }
}

fn collect_portability(node: &Node, path: &str, out: &mut Vec<String>) {
    match node {
        Node::Raw(_) => out.push(format!("{path}: Node::Raw is server-only")),
        Node::Native(_) => out.push(format!(
            "{path}: Node::Native is client-only — use Node::each() to ship this list"
        )),
        Node::Text(e) => {
            if !e.is_portable() {
                out.push(format!("{path}: Expr::Native is client-only"));
            }
        }
        Node::Fragment(children) => {
            for (i, c) in children.iter().enumerate() {
                collect_portability(c, &format!("{path}[{i}]"), out);
            }
        }
        Node::Element(el) => {
            for (i, a) in el.attrs.iter().enumerate() {
                if let Some(e) = a.expr() {
                    if !e.is_portable() {
                        out.push(format!("{path}@{}: Expr::Native is client-only", a.name));
                    }
                }
                if let AttrValue::Ref(_) = &a.value {
                    // Refs are names, not values: fully portable.
                }
                let _ = i;
            }
            for (i, c) in el.children.iter().enumerate() {
                collect_portability(c, &format!("{path}/{}.{i}", el.tag), out);
            }
        }
        Node::If {
            test,
            then,
            otherwise,
        } => {
            if !test.is_portable() {
                out.push(format!("{path}: if-test uses Expr::Native"));
            }
            collect_portability(then, &format!("{path}?then"), out);
            collect_portability(otherwise, &format!("{path}?else"), out);
        }
        Node::Show {
            when,
            children,
            fallback,
        } => {
            if !when.is_portable() {
                out.push(format!("{path}: show-when uses Expr::Native"));
            }
            collect_portability(children, &format!("{path}?content"), out);
            collect_portability(fallback, &format!("{path}?fallback"), out);
        }
        Node::Each { items, key, body } => {
            if !items.is_portable() {
                out.push(format!("{path}: each-items uses Expr::Native"));
            }
            if key.as_ref().is_some_and(|k| !k.is_portable()) {
                out.push(format!("{path}: each-key uses Expr::Native"));
            }
            collect_portability(body, &format!("{path}/*"), out);
        }
        Node::Component {
            name,
            attrs,
            children,
        } => {
            for a in attrs {
                if a.expr().is_some_and(|e| !e.is_portable()) {
                    out.push(format!("{path}<{name} {}>: Expr::Native", a.name));
                }
            }
            for (i, c) in children.iter().enumerate() {
                collect_portability(c, &format!("{path}<{name}>[{i}]"), out);
            }
        }
        Node::Missing | Node::Slot { .. } => {}
    }
}

// ---------------------------------------------------------------------------
// JSON round trip
// ---------------------------------------------------------------------------

#[cfg(feature = "serde")]
impl View {
    /// Serialize to JSON — the backend's half of the wire protocol.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Serialize to pretty-printed JSON, for payloads a human will read
    /// (an endpoint's debug view, a golden file in a test).
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse a payload from JSON — the client's half of the wire protocol.
    ///
    /// Rejects a payload whose [`AST_VERSION`] does not match, rather than
    /// rendering it incorrectly.
    pub fn from_json(json: &str) -> Result<View, ViewError> {
        let view: View = serde_json::from_str(json).map_err(ViewError::decode)?;
        if !crate::is_compatible_payload_version(view.version) {
            return Err(ViewError::VersionMismatch {
                found: view.version,
                expected: crate::AST_VERSION,
            });
        }
        Ok(view)
    }
}

/// Why a view could not be read from the wire.
#[derive(Debug)]
pub enum ViewError {
    /// The JSON was malformed or structurally wrong.
    Decode(String),
    /// The payload was written by an incompatible AST revision.
    VersionMismatch {
        /// The revision found in the payload.
        found: u16,
        /// The revision this build understands.
        expected: u16,
    },
}

impl ViewError {
    /// Attach a decode error. Only meaningful with the `serde` feature.
    #[cfg(feature = "serde")]
    fn decode(e: serde_json::Error) -> Self {
        ViewError::Decode(e.to_string())
    }
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViewError::Decode(msg) => write!(f, "view payload decode failed: {msg}"),
            ViewError::VersionMismatch { found, expected } => write!(
                f,
                "view payload is AST revision {found}, but this build expects {expected}"
            ),
        }
    }
}

impl std::error::Error for ViewError {}
//! Event bindings: what happens when a DOM event fires.
//!
//! An event action is an ordered list of [`EventOp`]s. Splitting it into small,
//! declarative ops (rather than a single opaque closure) is what makes an action
//! **portable**: the same op list runs in the browser and can be inspected,
//! logged, diffed, or server-rendered as a stub. Only [`EventOp::Call`] needs
//! code, and that code lives in a named handler table — either on the client or,
//! for a feature authored on a backend, on the server behind a route.

use crate::expr::{ArithOp, Expr};
use crate::expr::SignalId;

/// Identifies a handler in the view's [`HandlerRef`](crate::HandlerRef) table.
///
/// The id is chosen by whoever builds the view, so the client and the backend
/// only have to agree on a string. A handler may be backed by a
/// [`ServerRoute`] — which is precisely the "write the feature once on the
/// backend" case: the backend owns the logic and registers the route, the AST
/// just names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct HandlerId(pub String);

impl HandlerId {
    /// The handler name as a borrowed string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for HandlerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for HandlerId {
    fn from(v: &str) -> Self {
        HandlerId(v.to_string())
    }
}
impl From<String> for HandlerId {
    fn from(v: String) -> Self {
        HandlerId(v)
    }
}

/// A server endpoint an event can defer to.
///
/// The client issues the request, decodes the JSON body, and writes it into the
/// handler's declared signal — so a backend-defined feature gets full
/// fine-grained reactivity in the browser without shipping any of its logic to
/// the client.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ServerRoute {
    /// HTTP method. Defaults to `POST` when constructed via
    /// [`ServerRoute::post`].
    pub method: String,
    /// Request path, e.g. `/api/v1/firewall/rules/42/disable`.
    pub path: String,
    /// Extra `Content-Type: application/json` request body, as a template over
    /// [`ServerRoute::path`]'s captured state. `None` sends no body.
    ///
    /// Built by [`crate::view::HandlerRef`] rather than hand-written; see
    /// [`View::to_json`](crate::View::to_json) for how it is transported.
    pub body: Option<Expr>,
    /// Query parameters appended to `path`.
    pub query: Vec<(String, Expr)>,
}

impl ServerRoute {
    /// A `POST` to `path` with no body — the common "mutate this row" shape.
    pub fn post(path: impl Into<String>) -> Self {
        ServerRoute {
            method: "POST".to_string(),
            path: path.into(),
            body: None,
            query: Vec::new(),
        }
    }

    /// A `GET` from `path` — the common "load this panel" shape.
    pub fn get(path: impl Into<String>) -> Self {
        ServerRoute {
            method: "GET".to_string(),
            path: path.into(),
            body: None,
            query: Vec::new(),
        }
    }

    /// Override the HTTP method.
    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = method.into().to_uppercase();
        self
    }

    /// Send `body` as JSON.
    pub fn body(mut self, body: Expr) -> Self {
        self.body = Some(body);
        self
    }

    /// Append one query parameter.
    pub fn query_param(mut self, key: impl Into<String>, value: Expr) -> Self {
        self.query.push((key.into(), value));
        self
    }

    /// The full request URL with query parameters interpolated.
    pub fn url(&self, r: &dyn crate::expr::Resolver) -> String {
        if self.query.is_empty() {
            return self.path.clone();
        }
        let mut out = self.path.clone();
        for (i, (k, v)) in self.query.iter().enumerate() {
            out.push(if i == 0 && !out.contains('?') { '?' } else { '&' });
            out.push_str(k);
            out.push('=');
            out.push_str(&encode_component(&v.eval(r).to_text()));
        }
        out
    }
}

/// Percent-encode a query-string component. Deliberately minimal: the space and
/// the handful of characters that actually break a query string.
fn encode_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Where an event-sourced value comes from.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "k", content = "d"))]
pub enum EventValue {
    /// A static expression evaluated at dispatch time.
    Expr(Expr),
    /// A DOM property of the element the event fired on. The common cases are
    /// `value` (inputs, textareas, selects) and `checked` (checkboxes, radios).
    TargetProperty(String),
    /// The event target's `value` as text, or its text content when it has no
    /// value property.
    TargetText,
    /// The decoded JSON body of the most recent [`ServerRoute`] response in this
    /// same action. Yields null when no server call preceded it.
    Response,
}

/// One step of an event action.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "op", content = "v"))]
pub enum EventOp {
    /// Replace a signal's value.
    Set {
        /// Signal to write.
        target: SignalId,
        /// New value.
        value: EventValue,
    },
    /// Apply arithmetic to a signal: `Update { op: Add, by: 1 }` is `count + 1`.
    ///
    /// Pure WASM — no network, no allocation, no re-render of unrelated nodes.
    Update {
        /// Signal to update in place.
        target: SignalId,
        /// Which operator.
        op: ArithOp,
        /// The right-hand operand.
        by: EventValue,
    },
    /// Flip a signal used as a boolean. Matches the draft AST's
    /// `ToggleSignal`, generalized from `bool` to any value so a tri-state or
    /// integer toggle is expressible.
    Toggle {
        /// Signal to flip.
        target: SignalId,
    },
    /// Append to a list signal. `null` entries are dropped so a blank input does
    /// not accumulate empty rows.
    Push {
        /// List signal to push onto.
        target: SignalId,
        /// Value to append.
        value: EventValue,
    },
    /// Reset a signal to its declared initial value (the empty list for lists).
    Clear {
        /// Signal to reset.
        target: SignalId,
    },
    /// Invoke a handler — client-local or server-backed, depending on how the
    /// view declared it.
    Call {
        /// Handler to invoke.
        handler: crate::event::HandlerId,
    },
    /// Client-side navigation. `Expr::Route` re-renders the current route.
    Navigate {
        /// Destination path.
        to: Expr,
    },
    /// Run `ops` at most once per `ms`, cancelling any pending run. Turns a
    /// `keyup` handler into a debounced search with no extra machinery.
    Debounce {
        /// Quiet period in milliseconds.
        ms: u32,
        /// The delayed operations.
        ops: Vec<EventOp>,
    },
    /// Do **not** call `preventDefault()`.
    ///
    /// Every other op prevents it, matching the framework's `on:submit` sugar so
    /// a handled event never triggers a full page load. Opt out here when the
    /// browser's own behaviour is what you want (a real form post, a file
    /// download, opening a link in a new tab).
    AllowDefault,
}

impl EventOp {
    /// `count += by`
    pub fn add(target: impl Into<SignalId>, by: f64) -> EventOp {
        EventOp::Update {
            target: target.into(),
            op: ArithOp::Add,
            by: EventValue::Expr(Expr::Num(by)),
        }
    }

    /// `count -= by`
    pub fn sub(target: impl Into<SignalId>, by: f64) -> EventOp {
        EventOp::Update {
            target: target.into(),
            op: ArithOp::Sub,
            by: EventValue::Expr(Expr::Num(by)),
        }
    }

    /// Flip a boolean signal.
    pub fn toggle(target: impl Into<SignalId>) -> EventOp {
        EventOp::Toggle {
            target: target.into(),
        }
    }

    /// `signal = value`
    pub fn set(target: impl Into<SignalId>, value: EventValue) -> EventOp {
        EventOp::Set {
            target: target.into(),
            value,
        }
    }

    /// Invoke a handler by name.
    pub fn call(handler: impl Into<crate::event::HandlerId>) -> EventOp {
        EventOp::Call {
            handler: handler.into(),
        }
    }

    /// Every signal id this op (or any nested op) writes.
    pub fn write_targets(&self, out: &mut Vec<SignalId>) {
        match self {
            EventOp::Set { target, .. }
            | EventOp::Update { target, .. }
            | EventOp::Toggle { target }
            | EventOp::Push { target, .. }
            | EventOp::Clear { target } => {
                if !out.iter().any(|s| s == target) {
                    out.push(target.clone());
                }
            }
            EventOp::Call { .. } | EventOp::Navigate { .. } | EventOp::AllowDefault => {}
            EventOp::Debounce { ops, .. } => {
                for op in ops {
                    op.write_targets(out);
                }
            }
        }
    }

    /// Every handler this op (or any nested op) invokes.
    pub fn handler_calls(&self, out: &mut Vec<crate::event::HandlerId>) {
        match self {
            EventOp::Call { handler } => {
                if !out.iter().any(|h| h == handler) {
                    out.push(handler.clone());
                }
            }
            EventOp::Debounce { ops, .. } => {
                for op in ops {
                    op.handler_calls(out);
                }
            }
            _ => {}
        }
    }
}

/// What an element does when an event fires: an ordered op list.
///
/// ```
/// use velo_ast::{EventAction, EventOp};
/// use velo_ast::expr::ArithOp;
///
/// let action = EventAction::new(vec![
///     EventOp::add("count", 1.0),
///     EventOp::Call { handler: "persist".into() },
/// ]);
/// assert_eq!(action.write_targets(), vec!["count".to_string()]);
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct EventAction {
    /// The steps, applied in order.
    pub ops: Vec<EventOp>,
}

impl EventAction {
    /// An action from a list of ops.
    pub fn new(ops: Vec<EventOp>) -> Self {
        EventAction { ops }
    }

    /// A single-op action.
    pub fn one(op: EventOp) -> Self {
        EventAction { ops: vec![op] }
    }

    /// `true` when the action prevents the browser's default behaviour (i.e. it
    /// contains no [`EventOp::AllowDefault`]).
    pub fn prevents_default(&self) -> bool {
        !self.ops.iter().any(|op| matches!(op, EventOp::AllowDefault))
    }

    /// Signals written by this action, in first-visit order.
    pub fn write_targets(&self) -> Vec<SignalId> {
        let mut out = Vec::new();
        for op in &self.ops {
            op.write_targets(&mut out);
        }
        out
    }

    /// Handlers invoked by this action, in first-visit order.
    pub fn handler_calls(&self) -> Vec<crate::event::HandlerId> {
        let mut out = Vec::new();
        for op in &self.ops {
            op.handler_calls(&mut out);
        }
        out
    }

    /// `true` when any op may issue a network request (a handler backed by a
    /// [`ServerRoute`], resolved via the view's handler table).
    pub fn needs_network(&self, view: &crate::View) -> bool {
        self.handler_calls()
            .iter()
            .any(|h| view.handler(h.as_str()).is_some_and(|hr| hr.server.is_some()))
    }
}

/// One `on:<event>` attachment on an element.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EventBinding {
    /// The DOM event name (`click`, `input`, `submit`, …).
    pub event: String,
    /// What to run.
    pub action: EventAction,
}

impl EventBinding {
    /// Bind `action` to `event`.
    pub fn new(event: impl Into<String>, action: EventAction) -> Self {
        EventBinding {
            event: event.into(),
            action,
        }
    }
}
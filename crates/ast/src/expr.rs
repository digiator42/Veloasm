//! The expression tree: every value position in the AST.
//!
//! An [`Expr`] is *static structure*. Evaluating one against a [`Resolver`]
//! yields a [`Value`]. Expressions cover exactly the surface that can survive a
//! round trip to a browser — literals, signal reads with projections, static
//! payload reads, route/query params, interpolation, and a small arithmetic and
//! comparison core — plus one explicit escape hatch, [`Expr::Native`].

use std::fmt;
use std::rc::Rc;

use crate::path::ObjPath;
use crate::value::Value;

/// Stable identifier for a reactive cell, chosen by whoever *builds* the view.
///
/// The same string is used by the backend when it declares the signal, by the
/// client when it materializes the cell, and by every [`Expr`] that reads or
/// writes it. It is intentionally a string rather than an index: a backend
/// building a view in Rust should not have to thread a counter through
/// unrelated modules, and a string survives a JSON round trip without
/// renumbering.
pub type SignalId = String;

/// Arithmetic operators, shared by [`Expr::Arith`] and [`EventOp`] so an
/// increment action and an `a + 1` binding cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ArithOp {
    /// `+` — numeric addition, or string concatenation when either side is a
    /// string.
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/` — division by zero yields `0` rather than `inf`/`NaN`, so a bad
    /// payload can never poison the DOM with `NaN`.
    Div,
    /// `%`
    Rem,
}

impl ArithOp {
    /// Apply the operator to two already-evaluated values.
    pub fn apply(self, l: &Value, r: &Value) -> Value {
        match self {
            // `+` is the one polymorphic operator: string-ish wins over numeric,
            // which is what every template language does.
            ArithOp::Add if is_stringy(l) || is_stringy(r) => {
                Value::Str(format!("{}{}", l.to_text(), r.to_text()))
            }
            ArithOp::Add | ArithOp::Sub | ArithOp::Mul | ArithOp::Div | ArithOp::Rem => {
                let a = l.as_f64().unwrap_or(0.0);
                let b = r.as_f64().unwrap_or(0.0);
                let out = match self {
                    ArithOp::Add => a + b,
                    ArithOp::Sub => a - b,
                    ArithOp::Mul => a * b,
                    ArithOp::Div => {
                        if b == 0.0 {
                            0.0
                        } else {
                            a / b
                        }
                    }
                    // Guarded rather than `unreachable!()`: this arm is
                    // reachable if the string branch above is ever removed, and
                    // a silent wrong number beats a panic in a render loop.
                    ArithOp::Rem => {
                        if b == 0.0 {
                            0.0
                        } else {
                            a % b
                        }
                    }
                };
                Value::Num(out)
            }
        }
    }
}

/// Comparison operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum CmpOp {
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<` — lexicographic for strings, numeric otherwise.
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

impl CmpOp {
    /// Compare two evaluated values. Numeric comparison when both sides look
    /// numeric, string comparison otherwise; mixed operands fall back to string
    /// comparison rather than erroring.
    pub fn apply(self, l: &Value, r: &Value) -> Value {
        let both_numeric = matches!(l, Value::Num(_) | Value::Bool(_))
            && matches!(r, Value::Num(_) | Value::Bool(_));
        let out = if both_numeric {
            let (a, b) = (l.as_f64().unwrap_or(0.0), r.as_f64().unwrap_or(0.0));
            match self {
                CmpOp::Eq => a == b,
                CmpOp::Ne => a != b,
                CmpOp::Lt => a < b,
                CmpOp::Le => a <= b,
                CmpOp::Gt => a > b,
                CmpOp::Ge => a >= b,
            }
        } else {
            let (a, b) = (l.to_text(), r.to_text());
            match self {
                CmpOp::Eq => a == b,
                CmpOp::Ne => a != b,
                CmpOp::Lt => a < b,
                CmpOp::Le => a <= b,
                CmpOp::Gt => a > b,
                CmpOp::Ge => a >= b,
            }
        };
        Value::Bool(out)
    }
}

fn is_stringy(v: &Value) -> bool {
    matches!(v, Value::Str(_))
}

/// The ambient state an [`Expr`] reads from.
///
/// **Reactivity contract:** `signal()` is the *only* method a runtime may
/// implement in terms of its reactive engine. The client implementation calls
/// `RwSignal::get()`, which subscribes the running effect — so an expression
/// that reaches a signal becomes a dependency automatically, with no explicit
/// subscribe step. Implementations must not cache across calls without
/// subscribing.
///
/// `payload`, `route_param`, `query`, and `route` read immutable state, so they
/// are *not* dependencies; a view that needs them to be reactive must express
/// that with a signal.
pub trait Resolver {
    /// Read a signal, subscribing the running effect if there is one.
    fn signal(&self, id: &str) -> Option<Value>;
    /// Read the view's static (non-reactive) payload.
    fn payload(&self, path: &ObjPath) -> Option<Value>;
    /// Read a route parameter (`/blog/:slug` -> `slug`).
    fn route_param(&self, name: &str) -> Option<String>;
    /// Read a query-string parameter.
    fn query(&self, name: &str) -> Option<String>;
    /// The current client path.
    fn route(&self) -> String;

    /// Read the item bound by an enclosing [`Node::Each`](crate::Node::Each)
    /// loop. Defaults to "no such binding"; both renderers override it.
    fn loop_item(&self, _depth: u16) -> Option<Value> {
        None
    }

    /// The index of an enclosing [`Node::Each`](crate::Node::Each) loop.
    /// Defaults to "no such binding"; both renderers override it.
    fn loop_index(&self, _depth: u16) -> Option<usize> {
        None
    }
}

/// A resolver that knows nothing. Useful for SSR of a view with no runtime
/// state, and as the base for a resolver that overrides only what it needs.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullResolver;

impl Resolver for NullResolver {
    fn signal(&self, _id: &str) -> Option<Value> {
        None
    }
    fn payload(&self, _path: &ObjPath) -> Option<Value> {
        None
    }
    fn route_param(&self, _name: &str) -> Option<String> {
        None
    }
    fn query(&self, _name: &str) -> Option<String> {
        None
    }
    fn route(&self) -> String {
        String::new()
    }
    fn loop_item(&self, _depth: u16) -> Option<Value> {
        None
    }
    fn loop_index(&self, _depth: u16) -> Option<usize> {
        None
    }
}

/// A client-only expression: arbitrary Rust, evaluated in the browser.
///
/// This is the escape hatch that lets `vhtml!` splice a live Rust value
/// (`@(count.get())`) into a position the serializable [`Expr`] grammar cannot
/// express — a function call, a method chain, a computed `Vec`.
///
/// It is **never** serialized (see [`Node::is_portable`](crate::Node::is_portable)).
/// A `View` containing one can still be rendered in-process on the client, but
/// it cannot be shipped as JSON or server-rendered — which is the honest
/// consequence, and exactly what `is_portable` lets a backend assert before it
/// tries.
#[derive(Clone)]
pub struct NativeExpr(Rc<dyn Fn(&dyn Resolver) -> Value + 'static>);

impl NativeExpr {
    /// Wrap a closure as a native expression.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&dyn Resolver) -> Value + 'static,
    {
        NativeExpr(Rc::new(f))
    }

    /// Wrap a closure that ignores the resolver.
    pub fn from_static<F>(f: F) -> Self
    where
        F: Fn() -> Value + 'static,
    {
        NativeExpr(Rc::new(move |_| f()))
    }

    /// Evaluate the closure.
    pub fn eval(&self, r: &dyn Resolver) -> Value {
        (self.0)(r)
    }
}

impl fmt::Debug for NativeExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NativeExpr(..)")
    }
}

impl PartialEq for NativeExpr {
    /// Identity comparison: two native expressions are equal only if they wrap
    /// the same allocation. Structural equality is impossible, and tests compare
    /// by identity.
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Default for NativeExpr {
    /// Yields `Null`. Only ever constructed to satisfy `Default` bounds; a
    /// deserialized [`Expr::Native`] never keeps this value, because
    /// `NativeExpr`'s `Deserialize` impl always errors.
    fn default() -> Self {
        NativeExpr::from_static(Value::default)
    }
}

// `NativeExpr` is client-only. These impls exist so a *non-portable* view fails
// to serialize loudly rather than shipping a placeholder that renders empty.
#[cfg(feature = "serde")]
impl serde::Serialize for NativeExpr {
    fn serialize<S: serde::Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom(
            "Expr::Native is client-only and cannot be serialized — check \
             View::is_portable() before shipping this view",
        ))
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for NativeExpr {
    fn deserialize<D: serde::Deserializer<'de>>(_d: D) -> Result<Self, D::Error> {
        Err(serde::de::Error::custom(
            "NativeExpr is client-only and cannot be deserialized",
        ))
    }
}

/// A value position in the AST.
///
/// Construct these with the inherent helpers ([`Expr::str`], [`Expr::signal`],
/// …) rather than the variants — the helpers keep call sites short and make
/// refactors of the enum cheap.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "e", content = "v"))]
pub enum Expr {
    /// Renders nothing and, in attribute position, removes the attribute.
    Missing,
    /// Explicit null.
    Null,
    /// A boolean literal.
    Bool(bool),
    /// A number literal.
    Num(f64),
    /// A string literal.
    Str(String),
    /// Reactive read of a signal, optionally projected into its value.
    Signal {
        /// Which signal to read.
        id: SignalId,
        /// Projection applied after the read (empty = the value itself).
        path: ObjPath,
    },
    /// Non-reactive read of the view's static payload.
    Payload(ObjPath),
    /// Read a route parameter.
    RouteParam(String),
    /// Read a query-string parameter.
    Query(String),
    /// The item bound by the [`Node::Each`](crate::Node::Each) loop `depth`
    /// levels out, where `0` is the innermost enclosing loop.
    ///
    /// Depth-indexed rather than a bare "current item" so nested loops compose:
    /// `Each { items: Outer, body: Each { items: LoopItem(0), body: … } }`
    /// reaches the inner row with `LoopItem(0)` and the outer with
    /// `LoopItem(1)`.
    LoopItem(u16),
    /// The zero-based index of the enclosing loop `depth` levels out.
    LoopIndex(u16),
    /// The current client path.
    Route,
    /// An inline list.
    List(Vec<Expr>),
    /// An inline map. Ordered, so rendered output is deterministic.
    Map(Vec<(String, Expr)>),
    /// Interpolation: evaluate each part, join with `sep`. With an empty `sep`
    /// this is string concatenation / `"Hello, {name}"` style templating.
    Format {
        /// The pieces to render.
        parts: Vec<Expr>,
        /// The string placed between consecutive parts.
        sep: String,
    },
    /// Ternary. Evaluates exactly one branch.
    Cond {
        /// The condition; uses [`Value::truthy`].
        test: Box<Expr>,
        /// Taken when `test` is truthy.
        then: Box<Expr>,
        /// Taken otherwise. May itself be [`Expr::Missing`].
        otherwise: Box<Expr>,
    },
    /// Field access on a list or map value.
    Member {
        /// The value to index into.
        base: Box<Expr>,
        /// Field name.
        key: String,
    },
    /// Indexed access with a computed key (list index or map key).
    Index {
        /// The value to index into.
        base: Box<Expr>,
        /// Dynamic key.
        key: Box<Expr>,
    },
    /// First non-null operand. The `a ?? b` of the AST — essential for
    /// optional server data that may legitimately be absent.
    Coalesce(Vec<Expr>),
    /// Logical not.
    Not(Box<Expr>),
    /// Logical and; short-circuits and yields a boolean.
    And(Vec<Expr>),
    /// Logical or; short-circuits and yields a boolean.
    Or(Vec<Expr>),
    /// Comparison.
    Cmp {
        /// Which comparison.
        op: CmpOp,
        /// Left operand.
        l: Box<Expr>,
        /// Right operand.
        r: Box<Expr>,
    },
    /// Arithmetic.
    Arith {
        /// Which operator.
        op: ArithOp,
        /// Left operand.
        l: Box<Expr>,
        /// Right operand.
        r: Box<Expr>,
    },
    /// Arithmetic negation.
    Neg(Box<Expr>),
    /// Client-only escape hatch. **Not** `serde(skip)`: a skipped field would
    /// serialize as a bare `{"e":"Native"}` and deserialize into a `NativeExpr`
    /// that silently yields null — a view would ship, hydrate, and quietly render
    /// nothing. Serializing one instead fails with a diagnostic naming the
    /// problem, so [`View::is_portable`](crate::View::is_portable) is never
    /// bypassed by accident.
    Native(NativeExpr),
}

impl Expr {
    // -- constructors -------------------------------------------------------

    /// A string literal.
    pub fn str(s: impl Into<String>) -> Expr {
        Expr::Str(s.into())
    }

    /// A number literal.
    pub fn num(n: impl Into<f64>) -> Expr {
        Expr::Num(n.into())
    }

    /// A reactive read of a whole signal.
    pub fn signal(id: impl Into<SignalId>) -> Expr {
        Expr::Signal {
            id: id.into(),
            path: ObjPath::root(),
        }
    }

    /// A reactive read of `signal_id` projected through `path`.
    ///
    /// ```rust
    /// use velo_ast::{Expr, ObjPath};
    /// let e = Expr::signal_at("threats", ObjPath::parse("0.name").unwrap());
    /// ```
    pub fn signal_at(id: impl Into<SignalId>, path: ObjPath) -> Expr {
        Expr::Signal { id: id.into(), path }
    }

    /// Interpolation with an empty separator (concatenation).
    pub fn concat(parts: Vec<Expr>) -> Expr {
        Expr::Format { parts, sep: String::new() }
    }

    /// Join parts with `sep`.
    pub fn join(parts: Vec<Expr>, sep: impl Into<String>) -> Expr {
        Expr::Format { parts, sep: sep.into() }
    }

    /// `test ? then : otherwise`.
    pub fn cond(test: Expr, then: Expr, otherwise: Expr) -> Expr {
        Expr::Cond {
            test: Box::new(test),
            then: Box::new(then),
            otherwise: Box::new(otherwise),
        }
    }

    /// A client-only splice.
    pub fn native<F>(f: F) -> Expr
    where
        F: Fn(&dyn Resolver) -> Value + 'static,
    {
        Expr::Native(NativeExpr::new(f))
    }

    /// The item bound by the loop `depth` levels out (0 = innermost).
    pub fn item(depth: u16) -> Expr {
        Expr::LoopItem(depth)
    }

    /// The index of the loop `depth` levels out (0 = innermost).
    pub fn index(depth: u16) -> Expr {
        Expr::LoopIndex(depth)
    }

    /// A field of the item bound by the loop `depth` levels out — the shape
    /// that dominates a table template.
    pub fn item_field(depth: u16, key: impl Into<String>) -> Expr {
        Expr::Member {
            base: Box::new(Expr::LoopItem(depth)),
            key: key.into(),
        }
    }

    /// A field of an indexed list — `items[0].name`.
    pub fn at(base: Expr, index: usize) -> Expr {
        Expr::Index {
            base: Box::new(base),
            key: Box::new(Expr::Num(index as f64)),
        }
    }

    /// A field of any base expression — `base.name`.
    pub fn field(base: Expr, key: impl Into<String>) -> Expr {
        Expr::Member {
            base: Box::new(base),
            key: key.into(),
        }
    }

    /// A comparison of two expressions.
    ///
    /// `op` is [`CmpOp`] and the operands compare with `==`/`Ord` semantics:
    /// numbers numerically, strings lexicographically, and a composite value
    /// against a scalar as unequal — so a comparison never silently succeeds
    /// because two different shapes stringified the same way.
    pub fn cmp(op: CmpOp, l: Expr, r: Expr) -> Expr {
        Expr::Cmp {
            op,
            l: Box::new(l),
            r: Box::new(r),
        }
    }

    /// `left == right`.
    pub fn eq(left: Expr, right: Expr) -> Expr {
        Expr::cmp(CmpOp::Eq, left, right)
    }

    /// `left != right`.
    pub fn ne(left: Expr, right: Expr) -> Expr {
        Expr::cmp(CmpOp::Ne, left, right)
    }

    /// Arithmetic over two expressions.
    pub fn arith(op: ArithOp, l: Expr, r: Expr) -> Expr {
        Expr::Arith {
            op,
            l: Box::new(l),
            r: Box::new(r),
        }
    }

    /// The client-only closure behind an [`Expr::Native`], if this is one.
    pub fn native_ref(&self) -> Option<&NativeExpr> {
        match self {
            Expr::Native(n) => Some(n),
            _ => None,
        }
    }

    /// A client-only splice that renders directly to a string.
    ///
    /// This is what `vhtml!`'s `(expr)` syntax compiles to: it is the shortest
    /// honest path from "some Rust value" to "some text in the DOM", and it
    /// needs no intermediate [`Value`].
    ///
    /// ```
    /// use velo_ast::Expr;
    ///
    /// let count = 41;
    /// let e = Expr::render(move |_| format!("count is {count}"));
    /// assert_eq!(e.to_string(), "count is 41");
    /// ```
    pub fn render<F>(f: F) -> Expr
    where
        F: Fn(&dyn Resolver) -> String + 'static,
    {
        Expr::Native(NativeExpr::new(move |r| Value::Str(f(r))))
    }

    /// A client-only splice that yields a list.
    ///
    /// Backs `vhtml!`'s `@for`: the iterator is materialised per evaluation, so
    /// a loop over a signal stays live.
    pub fn native_list<F, I>(f: F) -> Expr
    where
        F: Fn(&dyn Resolver) -> I + 'static,
        I: IntoIterator,
        I::Item: Into<Value>,
    {
        Expr::Native(NativeExpr::new(move |r| {
            Value::List(f(r).into_iter().map(Into::into).collect())
        }))
    }

    /// A client-only boolean test — the condition of an `if` or a `Show`.
    pub fn test<F>(f: F) -> Expr
    where
        F: Fn(&dyn Resolver) -> bool + 'static,
    {
        Expr::Native(NativeExpr::new(move |r| Value::Bool(f(r))))
    }

    // -- analysis -----------------------------------------------------------

    /// `true` when this expression can be evaluated without a [`Resolver`] —
    /// i.e. it contains no signal, route, or payload read.
    ///
    /// Both renderers use this as a fast-path predicate: a static attribute is
    /// written once instead of being wrapped in a lifetime-long effect.
    pub fn is_static(&self) -> bool {
        match self {
            Expr::Missing
            | Expr::Null
            | Expr::Bool(_)
            | Expr::Num(_)
            | Expr::Str(_)
            | Expr::Payload(_)
            | Expr::RouteParam(_)
            | Expr::Query(_)
            | Expr::Route => true,
            // A loop binding needs the resolver *and* the row's own effect:
            // a row's content is not fixed at build time, so it must never take
            // the one-shot path.
            Expr::LoopItem(_) | Expr::LoopIndex(_) => false,
            Expr::Signal { .. } | Expr::Native(_) => false,
            Expr::List(items) => items.iter().all(Expr::is_static),
            Expr::Map(entries) => entries.iter().all(|(_, e)| e.is_static()),
            Expr::Format { parts, .. } => parts.iter().all(Expr::is_static),
            Expr::Cond { test, then, otherwise } => {
                test.is_static() && then.is_static() && otherwise.is_static()
            }
            Expr::Member { base, .. } => base.is_static(),
            Expr::Index { base, key } => base.is_static() && key.is_static(),
            Expr::Coalesce(parts) => parts.iter().all(Expr::is_static),
            Expr::Not(inner) => inner.is_static(),
            Expr::And(parts) | Expr::Or(parts) => parts.iter().all(Expr::is_static),
            Expr::Cmp { l, r, .. } | Expr::Arith { l, r, .. } => l.is_static() && r.is_static(),
            Expr::Neg(inner) => inner.is_static(),
        }
    }

    /// `true` when this expression reads reactive state, and therefore requires
    /// a live effect rather than a one-shot write.
    ///
    /// Signal reads and loop bindings both qualify: a keyed row's content is
    /// re-evaluated whenever its item changes, so a `LoopItem` field bound into
    /// an attribute must be an effect, not a fixed write.
    pub fn is_reactive(&self) -> bool {
        match self {
            Expr::Signal { .. } | Expr::LoopItem(_) | Expr::LoopIndex(_) => true,
            Expr::Native(_) => false,
            Expr::Missing
            | Expr::Null
            | Expr::Bool(_)
            | Expr::Num(_)
            | Expr::Str(_)
            | Expr::Payload(_)
            | Expr::RouteParam(_)
            | Expr::Query(_)
            | Expr::Route => false,
            Expr::List(items) => items.iter().any(Expr::is_reactive),
            Expr::Map(entries) => entries.iter().any(|(_, e)| e.is_reactive()),
            Expr::Format { parts, .. } => parts.iter().any(Expr::is_reactive),
            Expr::Cond { test, then, otherwise } => {
                test.is_reactive() || then.is_reactive() || otherwise.is_reactive()
            }
            Expr::Member { base, .. } => base.is_reactive(),
            Expr::Index { base, key } => base.is_reactive() || key.is_reactive(),
            Expr::Coalesce(parts) => parts.iter().any(Expr::is_reactive),
            Expr::Not(inner) => inner.is_reactive(),
            Expr::And(parts) | Expr::Or(parts) => parts.iter().any(Expr::is_reactive),
            Expr::Cmp { l, r, .. } | Expr::Arith { l, r, .. } => l.is_reactive() || r.is_reactive(),
            Expr::Neg(inner) => inner.is_reactive(),
        }
    }

    /// `true` if this expression can be serialized (no [`Expr::Native`]).
    pub fn is_portable(&self) -> bool {
        match self {
            Expr::Native(_) => false,
            Expr::Missing
            | Expr::Null
            | Expr::Bool(_)
            | Expr::Num(_)
            | Expr::Str(_)
            | Expr::Signal { .. }
            | Expr::Payload(_)
            | Expr::RouteParam(_)
            | Expr::Query(_)
            | Expr::Route
            | Expr::LoopItem(_)
            | Expr::LoopIndex(_) => true,
            Expr::List(items) => items.iter().all(Expr::is_portable),
            Expr::Map(entries) => entries.iter().all(|(_, e)| e.is_portable()),
            Expr::Format { parts, .. } => parts.iter().all(Expr::is_portable),
            Expr::Cond { test, then, otherwise } => {
                test.is_portable() && then.is_portable() && otherwise.is_portable()
            }
            Expr::Member { base, .. } => base.is_portable(),
            Expr::Index { base, key } => base.is_portable() && key.is_portable(),
            Expr::Coalesce(parts) => parts.iter().all(Expr::is_portable),
            Expr::Not(inner) => inner.is_portable(),
            Expr::And(parts) | Expr::Or(parts) => parts.iter().all(Expr::is_portable),
            Expr::Cmp { l, r, .. } | Expr::Arith { l, r, .. } => l.is_portable() && r.is_portable(),
            Expr::Neg(inner) => inner.is_portable(),
        }
    }

    /// The signal ids this expression reads, in first-visit order.
    pub fn signal_reads(&self, out: &mut Vec<SignalId>) {
        match self {
            Expr::Signal { id, .. } => {
                if !out.iter().any(|s| s == id) {
                    out.push(id.clone());
                }
            }
            Expr::Native(_) => {}
            Expr::Missing
            | Expr::Null
            | Expr::Bool(_)
            | Expr::Num(_)
            | Expr::Str(_)
            | Expr::Payload(_)
            | Expr::RouteParam(_)
            | Expr::Query(_)
            | Expr::Route
            | Expr::LoopItem(_)
            | Expr::LoopIndex(_) => {}
            Expr::List(items) => items.iter().for_each(|e| e.signal_reads(out)),
            Expr::Map(entries) => entries.iter().for_each(|(_, e)| e.signal_reads(out)),
            Expr::Format { parts, .. } => parts.iter().for_each(|e| e.signal_reads(out)),
            Expr::Cond { test, then, otherwise } => {
                test.signal_reads(out);
                then.signal_reads(out);
                otherwise.signal_reads(out);
            }
            Expr::Member { base, .. } => base.signal_reads(out),
            Expr::Index { base, key } => {
                base.signal_reads(out);
                key.signal_reads(out);
            }
            Expr::Coalesce(parts) => parts.iter().for_each(|e| e.signal_reads(out)),
            Expr::Not(inner) => inner.signal_reads(out),
            Expr::And(parts) | Expr::Or(parts) => parts.iter().for_each(|e| e.signal_reads(out)),
            Expr::Cmp { l, r, .. } | Expr::Arith { l, r, .. } => {
                l.signal_reads(out);
                r.signal_reads(out);
            }
            Expr::Neg(inner) => inner.signal_reads(out),
        }
    }

    /// The string this expression renders as, when it is a compile-time-known
    /// literal. Lets the renderer skip evaluation entirely for `class="card"`
    /// and similar.
    pub fn literal(&self) -> Option<String> {
        match self {
            Expr::Str(s) => Some(s.clone()),
            Expr::Num(_) => Some(self.eval(&NullResolver).to_text()),
            Expr::Bool(b) => Some(b.to_string()),
            Expr::Missing | Expr::Null => Some(String::new()),
            _ => None,
        }
    }

    // -- evaluation ---------------------------------------------------------

    /// Evaluate against a resolver.
    pub fn eval(&self, r: &dyn Resolver) -> Value {
        match self {
            Expr::Missing | Expr::Null => Value::Null,
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Num(n) => Value::Num(*n),
            Expr::Str(s) => Value::Str(s.clone()),
            Expr::Signal { id, path } => match r.signal(id) {
                Some(v) => {
                    if path.is_root() {
                        v
                    } else {
                        v.path(path).unwrap_or(Value::Null)
                    }
                }
                // An unknown signal id renders as null rather than panicking:
                // a stale client must never take down a whole page.
                None => Value::Null,
            },
            Expr::Payload(path) => r.payload(path).unwrap_or(Value::Null),
            Expr::RouteParam(name) => match r.route_param(name) {
                Some(s) => Value::Str(s),
                None => Value::Null,
            },
            Expr::Query(name) => match r.query(name) {
                Some(s) => Value::Str(s),
                None => Value::Null,
            },
            Expr::Route => Value::Str(r.route()),
            Expr::LoopItem(depth) => r.loop_item(*depth).unwrap_or(Value::Null),
            Expr::LoopIndex(depth) => r
                .loop_index(*depth)
                .map(|i| Value::Num(i as f64))
                .unwrap_or(Value::Null),
            Expr::List(items) => Value::List(items.iter().map(|e| e.eval(r)).collect()),
            Expr::Map(entries) => Value::Map(
                entries
                    .iter()
                    .map(|(k, v)| (k.clone(), v.eval(r)))
                    .collect(),
            ),
            Expr::Format { parts, sep } => {
                let mut out = String::new();
                for (i, p) in parts.iter().enumerate() {
                    if i > 0 {
                        out.push_str(sep);
                    }
                    out.push_str(&p.eval(r).to_text());
                }
                Value::Str(out)
            }
            Expr::Cond { test, then, otherwise } => {
                if test.eval(r).truthy() {
                    then.eval(r)
                } else {
                    otherwise.eval(r)
                }
            }
            Expr::Member { base, key } => index_value(&base.eval(r), key),
            Expr::Index { base, key } => {
                let b = base.eval(r);
                let k = key.eval(r);
                match (&b, &k) {
                    (Value::List(items), Value::Num(n)) => {
                        items.get(*n as usize).cloned().unwrap_or(Value::Null)
                    }
                    (Value::Map(_), k) => index_value(&b, &k.to_text()),
                    _ => Value::Null,
                }
            }
            Expr::Coalesce(parts) => {
                for p in parts {
                    let v = p.eval(r);
                    if !matches!(v, Value::Null) {
                        return v;
                    }
                }
                Value::Null
            }
            Expr::Not(inner) => Value::Bool(!inner.eval(r).truthy()),
            Expr::And(parts) => {
                for p in parts {
                    if !p.eval(r).truthy() {
                        return Value::Bool(false);
                    }
                }
                Value::Bool(true)
            }
            Expr::Or(parts) => {
                for p in parts {
                    if p.eval(r).truthy() {
                        return Value::Bool(true);
                    }
                }
                Value::Bool(false)
            }
            Expr::Cmp { op, l, r: rr } => op.apply(&l.eval(r), &rr.eval(r)),
            Expr::Arith { op, l, r: rr } => op.apply(&l.eval(r), &rr.eval(r)),
            Expr::Neg(inner) => Value::Num(-inner.eval(r).as_f64().unwrap_or(0.0)),
            Expr::Native(native) => native.eval(r),
        }
    }
}

/// Field access on a value: map key, or the single row of a one-element list.
fn index_value(base: &Value, key: &str) -> Value {
    match base {
        Value::Map(m) => m.get(key).cloned().unwrap_or(Value::Null),
        Value::List(items) if items.len() == 1 => index_value(&items[0], key),
        _ => Value::Null,
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.eval(&NullResolver).to_text())
    }
}

impl From<&str> for Expr {
    fn from(v: &str) -> Self {
        Expr::Str(v.to_string())
    }
}
impl From<String> for Expr {
    fn from(v: String) -> Self {
        Expr::Str(v)
    }
}
impl From<&String> for Expr {
    fn from(v: &String) -> Self {
        Expr::Str(v.clone())
    }
}
impl From<bool> for Expr {
    fn from(v: bool) -> Self {
        Expr::Bool(v)
    }
}
impl From<f64> for Expr {
    fn from(v: f64) -> Self {
        Expr::Num(v)
    }
}
impl From<f32> for Expr {
    fn from(v: f32) -> Self {
        Expr::Num(v as f64)
    }
}
impl From<i32> for Expr {
    fn from(v: i32) -> Self {
        Expr::Num(v as f64)
    }
}
impl From<i64> for Expr {
    fn from(v: i64) -> Self {
        Expr::Num(v as f64)
    }
}
impl From<usize> for Expr {
    fn from(v: usize) -> Self {
        Expr::Num(v as f64)
    }
}
impl From<ObjPath> for Expr {
    fn from(v: ObjPath) -> Self {
        Expr::Payload(v)
    }
}
//! The dynamically-typed runtime value produced by evaluating an [`Expr`].
//!
//! [`Value`] is deliberately *not* the same thing as [`Expr`]:
//!
//! - [`Expr`] is **static structure** — part of the AST, serializable, cheap.
//! - [`Value`] is **dynamic data** — the result of running an `Expr` against a
//!   [`Resolver`](crate::Resolver) at a point in time, never serialized itself
//!   (its contents travel inside a signal's `Expr` seed or a `View` payload).
//!
//! Keeping them separate is what lets a signal hold *arbitrary* server JSON
//! (a whole table of records) while the view tree that reads it stays a small,
//! predictable set of typed nodes.

use std::collections::BTreeMap;
use std::fmt;

use crate::path::ObjPath;

/// A dynamically-typed JSON-shaped value.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "t", content = "v"))]
pub enum Value {
    /// Absent / explicitly empty. Distinct from [`Value::Null`] only where the
    /// distinction matters for `MaybeUninit`-style attributes; both render
    /// empty in text position.
    #[default]
    Null,
    /// A boolean.
    Bool(bool),
    /// A number. Always `f64` so the wire format stays JSON-native.
    Num(f64),
    /// A UTF-8 string.
    Str(String),
    /// An ordered list.
    List(Vec<Value>),
    /// A string-keyed map. `BTreeMap`, not `HashMap`, so iteration — and
    /// therefore any rendered output — is deterministic.
    Map(BTreeMap<String, Value>),
}

impl Value {
    /// Build a map value from key/value pairs.
    pub fn map<K: Into<String>, I: IntoIterator<Item = (K, Value)>>(pairs: I) -> Self {
        Value::Map(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// The string payload, if this is a [`Value::Str`].
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The numeric payload, widened for [`Value::Num`] and coerced from bools.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    /// The integer payload, if this is a [`Value::Num`] with no fraction.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Num(n) if n.is_finite() && n.fract() == 0.0 => Some(*n as i64),
            _ => None,
        }
    }

    /// The boolean payload. Non-bool values use [`Value::truthy`], so
    /// `if={ some_string }` and `if={ flag }` behave the way a reader expects.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Truthiness for `if` / `when` / class-toggle conditions.
    ///
    /// Follows HTML-template convention rather than Rust's: an empty string,
    /// an empty list, and `0` are all **falsey**, `Null` is falsey, and
    /// every other value is truthy. `#[serde]`-round-tripped JSON therefore
    /// behaves identically in `vhtml!`, in SSR, and in the browser.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0.0 && !n.is_nan(),
            Value::Str(s) => !s.is_empty(),
            Value::List(v) => !v.is_empty(),
            Value::Map(m) => !m.is_empty(),
        }
    }

    /// The list payload, if this is a [`Value::List`].
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(v) => Some(v),
            _ => None,
        }
    }

    /// The map payload, if this is a [`Value::Map`].
    pub fn as_map(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    /// Follow an [`ObjPath`] through nested maps/lists. Returns `None` if any
    /// segment is missing or the shape does not match.
    pub fn path(&self, path: &ObjPath) -> Option<Value> {
        let mut cur = self;
        for seg in path.segments() {
            match seg {
                crate::path::PathSeg::Key(k) => match cur {
                    Value::Map(m) => cur = m.get(k)?,
                    // Reading a field off a list is sugar for "the single row".
                    Value::List(l) if l.len() == 1 => {
                        cur = l.first()?;
                        let Value::Map(m) = cur else { return None };
                        cur = m.get(k)?;
                    }
                    _ => return None,
                },
                crate::path::PathSeg::Index(i) => match cur {
                    Value::List(l) => cur = l.get(*i)?,
                    _ => return None,
                },
            }
        }
        Some(cur.clone())
    }

    /// Flatten to the string used in text position. Booleans and numbers use
    /// their natural display; `Null` renders as the empty string.
    pub fn to_text(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Num(n) => format_number(*n),
            Value::Str(s) => s.clone(),
            // Composite values in text position stringify as JSON so the SSR
            // output is at least debuggable rather than silently empty.
            Value::List(_) | Value::Map(_) => self.to_debug_json(),
        }
    }

    /// Lossless-enough JSON rendering, used for debug output and for turning a
    /// composite value into an attribute string. Not a substitute for a real
    /// serializer — use [`Value::from_json`] / [`Value::to_json`] for that.
    pub fn to_debug_json(&self) -> String {
        match self {
            Value::Null => "null".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Num(n) => format_number(*n),
            // Quoted and escaped. This feeds `JSON.parse` on the client — a
            // remote handler's request body is produced by `to_text` on a
            // composite — so a bare string here does not render slightly wrong,
            // it produces a body the client cannot parse and then sends nothing.
            Value::Str(s) => json_string(s),
            Value::List(items) => {
                let inner: Vec<String> = items.iter().map(Value::to_debug_json).collect();
                format!("[{}]", inner.join(","))
            }
            Value::Map(m) => {
                let inner: Vec<String> = m
                    .iter()
                    // The key is quoted too. `{q:1}` is not JSON, and the
                    // difference is invisible until a request silently loses its
                    // body.
                    .map(|(k, v)| format!("{}:{}", json_string(k), v.to_debug_json()))
                    .collect();
                format!("{{{}}}", inner.join(","))
            }
        }
    }

    /// Deep structural equality. Used by the client runtime to decide whether a
    /// reactive write actually changed anything (and can therefore be skipped).
    pub fn identical(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Num(a), Value::Num(b)) => a == b || (a.is_nan() && b.is_nan()),
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::List(a), Value::List(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.identical(y))
            }
            (Value::Map(a), Value::Map(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .zip(b.iter())
                        .all(|((ka, va), (kb, vb))| ka == kb && va.identical(vb))
            }
            _ => false,
        }
    }
}

/// Render an `f64` the way a human wrote it: `1` not `1.0`, but `1.5` stays
/// `1.5`. Keeps SSR output and `data-*` attributes free of trailing `.0`.
pub(crate) fn format_number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Quote and escape `s` as a JSON string.
///
/// The escape set is JSON's, not HTML's. `to_debug_json` is what produces a
/// remote handler's request body, and `JSON.parse` on the other end rejects an
/// unescaped `"` or a raw newline — so this is load-bearing, not cosmetic.
/// `<`, `>`, `&` are deliberately *not* escaped here: the result is JSON, not
/// markup, and `\u003c` in a request body helps nobody.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_text())
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.identical(other)
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Num(v)
    }
}
impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Value::Num(v as f64)
    }
}
impl From<usize> for Value {
    fn from(v: usize) -> Self {
        Value::Num(v as f64)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Num(v as f64)
    }
}
impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Num(v as f64)
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Str(v)
    }
}
impl From<&String> for Value {
    fn from(v: &String) -> Self {
        Value::Str(v.clone())
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Str(v.to_string())
    }
}
impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::List(v)
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        match v {
            Some(inner) => inner.into(),
            None => Value::Null,
        }
    }
}

// ---------------------------------------------------------------------------
// serde_json interop — the bridge that lets a Rust backend push its own
// already-serialized model rows straight into a signal.
// ---------------------------------------------------------------------------

#[cfg(feature = "serde")]
impl Value {
    /// Adopt a `serde_json::Value`. Object key order is **lost** (JSON objects
    /// become a `BTreeMap`), which is intentional: determinism over insertion
    /// order.
    pub fn from_json(v: &serde_json::Value) -> Value {
        match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(b) => Value::Bool(*b),
            serde_json::Value::Number(n) => Value::Num(n.as_f64().unwrap_or(f64::NAN)),
            serde_json::Value::String(s) => Value::Str(s.clone()),
            serde_json::Value::Array(a) => Value::List(a.iter().map(Value::from_json).collect()),
            serde_json::Value::Object(o) => {
                Value::Map(o.iter().map(|(k, v)| (k.clone(), Value::from_json(v))).collect())
            }
        }
    }

    /// Lower to a `serde_json::Value`.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Null => serde_json::Value::Null,
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Num(n) => serde_json_number(*n),
            Value::Str(s) => serde_json::Value::String(s.clone()),
            Value::List(v) => serde_json::Value::Array(v.iter().map(Value::to_json).collect()),
            Value::Map(m) => serde_json::Value::Object(
                m.iter().map(|(k, v)| (k.clone(), v.to_json())).collect(),
            ),
        }
    }
}

/// `serde_json::Number` cannot hold NaN/Inf, so non-finite values degrade to
/// `Null` rather than panicking or producing invalid JSON.
#[cfg(feature = "serde")]
pub(crate) fn serde_json_number(n: f64) -> serde_json::Value {
    if !n.is_finite() {
        return serde_json::Value::Null;
    }
    if n.fract() == 0.0 && n.abs() < 1e15 {
        serde_json::Value::Number(serde_json::Number::from(n as i64))
    } else {
        serde_json::Number::from_f64(n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)
    }
}
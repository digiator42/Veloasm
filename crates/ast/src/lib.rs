//! # `velo_ast` — the shared, serializable view AST
//!
//! One data structure, written **once** in Rust, rendered **twice**:
//!
//! ```text
//!                    ┌──────────────────────────────────────────┐
//!                    │        velo_ast::View  (pure data)        │
//!                    │  Node · Expr · Attr · EventBinding · ...  │
//!                    └───┬──────────────────────┬───────────────┘
//!          render_html() │                      │ deserialize + render()
//!         (native/SSR)  │                      │  (wasm client, veloasm)
//!          ┌────────────▼──────────┐   ┌───────▼────────────────┐
//!          │  "…<tr><td>Ada</td>"  │   │  fine-grained DomNode  │
//!          │   GritShield  / SSR   │   │   signal-backed DOM    │
//!          └───────────────────────┘   └────────────────────────┘
//! ```
//!
//! ## Why this crate exists
//!
//! `view!` (`velo_macro`) compiles straight to *imperative* Rust operating on
//! `DomNode`. That is ideal for a same-language client, but it means the view
//! **cannot leave the compiler** — there is no artifact a Rust backend can hand
//! to a browser. `velo_ast` is the missing middle: a plain `Serialize` data
//! structure that a backend (e.g. `gritshield`) can build, ship as JSON, and a
//! wasm client can mount with live fine-grained reactivity.
//!
//! ## Design rules
//!
//! 1. **Pure data.** No `DomNode`, no `web-sys`, no tokio. The same source must
//!    compile for `wasm32-unknown-unknown` and for a native server, so the
//!    default feature set has *zero* dependencies.
//! 2. **Deterministic.** Every collection that can reach the wire is a `Vec`,
//!    never a `HashMap` — SSR output and JSON payloads must be byte-stable so
//!    they diff, cache, and test cleanly.
//! 3. **Typed values, no untyped `String` soup.** `Expr` is an expression tree,
//!    not a format string, so `disabled={ false }` round-trips as a boolean and
//!    `class:active={ sig }` stays a distinct node kind.
//! 4. **Optional escape hatches.** [`Expr::Native`] and [`Node::Raw`] are
//!    explicitly client-only / server-only and are *not* serialized; see
//!    [`Node::is_portable`].
//!
//! ## Feature flags
//!
//! - `serde` (default **off**) — `Serialize`/`Deserialize` derives plus
//!   [`serde_json`] re-exports. Turn it on for the backend/JSON wire path. A
//!   pure `vhtml!`-compiled wasm app never pays for it.

#![deny(missing_docs)]

pub mod attr;
pub mod event;
pub mod expr;
pub mod html;
pub mod node;
pub mod parse_html;
pub mod path;
pub mod value;
pub mod view;

#[cfg(all(test, feature = "serde"))]
mod wire_tests;

#[cfg(feature = "serde")]
pub use serde;
#[cfg(feature = "serde")]
pub use serde_json;

pub use attr::{Attr, AttrValue};
pub use event::{
    EventAction, EventBinding, EventOp, EventValue, HandlerId, ServerRoute,
};
pub use expr::{ArithOp, CmpOp, Expr, NativeExpr, NullResolver, Resolver, SignalId};
pub use html::{
    escape_attr, escape_raw_text, escape_text, render, render_document, render_handler_manifest,
    render_node, render_partial, render_seed_island, render_with, HtmlOptions, SeedIsland,
};
pub use node::{Element, Node};
pub use path::{ObjPath, PathSeg};
pub use value::Value;
pub use view::{HandlerRef, SignalDecl, SignalKind, View, ViewError};

/// Bumped whenever a change to the wire format would make older payloads
/// unreadable by a newer client (or vice versa). Purely additive changes —
/// new optional fields, new `Expr`/`Node` variants — do **not** bump it.
pub const AST_VERSION: u16 = 1;

/// `true` when a JSON payload was produced by an incompatible AST revision and
/// should be rejected rather than silently mis-rendered.
pub fn is_compatible_payload_version(version: u16) -> bool {
    version == AST_VERSION
}
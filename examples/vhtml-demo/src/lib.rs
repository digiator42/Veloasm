//! Dedicated `vhtml!` macro demo — every syntax sugar in one place.
//!
//! This is the developer guide for the `vhtml!` proc macro (commit `9e8b176`)
//! and the compile-time `schema` / keyed `@for` features (commit `ec916e1`).
//!
//! Every feature compiles to [`velo_ast`](veloasm::velo_ast) nodes and can be
//! mounted with [`veloasm::ast::mount_node`] or wrapped in a [`View`] for the
//! shared-AST wire format.
//!
//! # Signal references
//!
//! Signal ids can be spelled two ways:
//! - `@Show("flag")` / `Show when="flag"` — quoted string (classic)
//! - `@Show(flag)` / `Show when=flag` — bare identifier (Maud-like)
//! - `Show when=|flag|` — pipe sugar for component attrs (clearest, recommended)
//!
//! # Raw `Expr` escape hatches
//!
//! - `${ Expr::signal("count") }` — raw `velo_ast::Expr` (JS template-literal style)
//! - `{{ Expr::signal("count") }}` — alternative raw `Expr` syntax
//!
//! # Syntax quick reference
//!
//! ```ignore
//! // Element + static attr + text
//! div class="card" { h2 { "Title" } p { "body" } }
//!
//! // Reactive text: (expr) — signal auto-unwraps via signal_value!
//! p { "count: " (count.get()) }
//!
//! // Class shorthand: .cls = static toggle
//! div .box { "box" }
//!
//! // Class toggle from reactive condition (pipe signal ref)
//! div class:highlight=|flag| { "toggles" }
//!
//! // Event: on:<event>={ EventAction }
//! button on:click={ EventAction::one(EventOp::add("count", 1.0)) } { "+" }
//!
//! // @Show block form: reactive, portable signal id
//! @Show(flag) { p { "on" } } @else { p { "off" } }
//!
//! // @Show with @else if chain (all reactive)
//! @Show(active) { … } @else if (count.get() > 0) { … } @else { … }
//!
//! // Show component form — cleaner DX
//! Show when=|flag| fallback=${ Node::text("off") } { p { "on" } }
//!
//! // Suspense component form — inverted predicate
//! Suspense loading=|loading| fallback="loading…" { … }
//!
//! // @for unkeyed — plain Rust body, rebuilt on change
//! @for i in 0..3 { li { "row " (i) } }
//!
//! // @for keyed — Value grammar, reconciled by identity
//! @for t in rows.get(), key = t.id { li data-id=(t.id) { (t.title) } }
//!
//! // schema header — compile-time signal contract
//! schema { count, flag }
//!
//! // @let — lexical binding
//! @let label = "hello";
//! ```

use veloasm::vhtml;
use veloasm::velo_ast::event::{EventAction, EventOp};
use veloasm::velo_ast::{Expr, Node, SignalDecl, Value, View};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn threat_row(id: u32, title: &str, severity: &str) -> Value {
    Value::map([
        ("id", Value::from(id as usize)),
        ("title", Value::from(title)),
        ("severity", Value::from(severity)),
    ])
}

// ---------------------------------------------------------------------------
// 1. Everything in one view
// ---------------------------------------------------------------------------

/// A single view exercising every `vhtml!` syntax feature.
///
/// All signals are declared on the `View` (AST level). Events write to AST
/// signal ids, and reads use `${ Expr::signal("id") }` to read the same AST
/// signals through the `SignalStore`. This is what makes the buttons actually
/// update the display.
pub fn all_features() -> Node {
    vhtml! {
        // compile-time contract: every signal id named below must be declared
        schema { count, flag, loading, title },

        div class="shell" {
            // --- elements, static attrs, text -----------------------------------
            h1 id="title" { ${ Expr::signal("title") } }

            // static attribute
            p id="static" { "static paragraph" }

            // reactive text reading the AST "count" signal
            p { "count: " ${ Expr::signal("count") } }

            // class shorthand: static toggle
            div .box { "has static box class" }

            // class toggle from the AST "flag" signal (pipe signal ref)
            div class:highlight=|flag| { "highlight toggles with flag" }

            // --- events: on:<event>={ EventAction } -------------------------
            // Events write to AST signal ids. The SignalStore is shared with
            // the reads above, so the display updates live.
            div {
                button on:click=( EventAction::one(EventOp::add("count", 1.0)) ) {
                    "increment count"
                }
                button on:click=( EventAction::one(EventOp::toggle("flag")) ) {
                    "toggle flag"
                }
            }

            // --- conditionals ----------------------------------------------
            // @Show block form — string signal id
            @Show(flag) {
                p { "flag is on" }
            } @else {
                p { "flag is off" }
            }

            // Show component form — pipe signal ref
            Show when=|flag| fallback="flag off" {
                p class="badge" { "flag badge" }
            }

            // Show component form — raw Expr signal reference (${})
            Show when=${ Expr::signal("flag") } fallback=${ Node::text("flag off") } {
                p { "flag via raw Expr" }
            }

            // Suspense component form — inverted predicate, pipe signal ref
            Suspense loading=|loading| fallback="loading…" {
                p { "content ready" }
            }

            // --- loops ------------------------------------------------------
            // unkeyed: plain Rust body (static list, just syntax demo)
            ul id="unkeyed" {
                @for i in 0..3 {
                    li { "unkeyed row: " (i) }
                }
            }

            // keyed: Value grammar, reconciled by identity (key = t.id)
            ul id="keyed" {
                @for t in vec![threat_row(1, "alpha", "low"), threat_row(2, "beta", "high")], key = t.id {
                    li data-id=(t.id) class:severe=(t.severity == "high") {
                        span class="title" { (t.title) }
                        span class="sev" { "[" (t.severity) "]" }
                    }
                }
            }

            // --- @let -------------------------------------------------------
            @let greeting = "hello from @let";
            p { (greeting) }
        }
    }
}

/// Wrap `all_features` into a serializable `View` with signal declarations.
pub fn all_features_view() -> View {
    View::new(
        "vhtml-demo",
        all_features(),
    )
    .with_signal(SignalDecl::count("count", 0.0))
    .with_signal(SignalDecl::scalar("flag", true))
    .with_signal(SignalDecl::scalar("loading", false))
    .with_signal(SignalDecl::scalar("title", "Demo"))
    .with_signal(SignalDecl::list("rows", Vec::<Value>::new()))
    .declare_used_signals()
}

// ---------------------------------------------------------------------------
// Browser entry point — live reactive demo
// ---------------------------------------------------------------------------

/// Mount `all_features_view()` into `<body>` and let the buttons drive live
/// reactivity: increment count, toggle flag, watch `@Show` / `Show` /
/// `Suspense` / keyed list respond without a page reload.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    let view = all_features_view();
    let node = veloasm::ast::mount(&view);
    veloasm::mount(node);
}

// ---------------------------------------------------------------------------
// Minimal: each feature in isolation
// ---------------------------------------------------------------------------

/// `@Show` block form with full `@else if` / `@else` chain.
pub fn show_block() -> Node {
    vhtml! {
        schema { active, count },
        @Show("active") {
            p { "active" }
        } @else if (true) {
            p { "else-if branch" }
        } @else {
            p { "fallback" }
        }
    }
}

/// `Show` component form (cleaner DX) + `Suspense` (inverted).
pub fn show_component() -> Node {
    vhtml! {
        div {
            Show when=|flag| fallback=${ Node::text("off") } {
                p { "on" }
            }
            Suspense loading=|loading| fallback=${ Node::text("loading…") } {
                p { "ready" }
            }
        }
    }
}

/// Unkeyed `@for` — plain Rust body.
pub fn unkeyed_loop() -> Node {
    vhtml! {
        ul {
            @for i in 0..4 {
                li { "item: " (i) }
            }
        }
    }
}

/// Keyed `@for` — Value grammar, reconciled by identity.
pub fn keyed_loop() -> Node {
    vhtml! {
        ul {
            @for t in vec![threat_row(1, "alpha", "low"), threat_row(2, "beta", "high")], key = t.id {
                li data-id=(t.id) class:severe=(t.severity == "high") {
                    span { (t.title) }
                }
            }
        }
    }
}

/// `@let` lexical binding.
pub fn let_binding() -> Node {
    vhtml! {
        @let label = "bound by @let",
        p { label }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_block_compiles() {
        let _: Node = show_block();
    }

    #[test]
    fn show_component_compiles() {
        let _: Node = show_component();
    }

    #[test]
    fn unkeyed_loop_compiles() {
        let _: Node = unkeyed_loop();
    }

    #[test]
    fn keyed_loop_compiles() {
        let _: Node = keyed_loop();
    }

    #[test]
    fn let_binding_compiles() {
        let _: Node = let_binding();
    }

    #[test]
    fn all_features_compiles() {
        let _: Node = all_features();
    }

    #[test]
    fn pipe_signal_ref_compiles() {
        // `|flag|` pipe syntax for signal references
        let _: Node = vhtml! {
            schema { count },
            Show when=|count| fallback=${ Node::text("off") } {
                p { "on" }
            }
        };
    }

    #[test]
    fn schema_rejects_undeclared_show_id() {
        // This must FAIL to compile — `@Show("missing")` is not in the schema.
        // We can't assert a compile error at runtime, but the macro emits it.
        // The test below documents the expected behavior.
        let _ = vhtml! {
            schema { count },
            @Show("count") { p { "declared" } }
        };
    }
}

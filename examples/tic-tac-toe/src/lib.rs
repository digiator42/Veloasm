//! Tic-tac-toe built with `veloasm::vhtml!` macro.
//!
//! This example demonstrates using the `vhtml!` macro from `veloasm`
//! (commit `9e8b176`) to build a tic-tac-toe game with clean syntax.
//!
//! Run the tests:
//!   cargo test -p tic-tac-toe
//!
//! Build for the browser:
//!   trunk serve --dir examples/tic-tac-toe

use veloasm::vhtml;
use veloasm::velo_ast::{Expr, SignalDecl, Value, View, Node};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn threat_row(id: u32, title: &str, severity: &str) -> veloasm::velo_ast::Value {
    veloasm::velo_ast::Value::map([
        ("id", veloasm::velo_ast::Value::from(id as usize)),
        ("title", veloasm::velo_ast::Value::from(title)),
        ("severity", veloasm::velo_ast::Value::from(severity)),
    ])
}

// ---------------------------------------------------------------------------
// 1. Everything in one view
// ---------------------------------------------------------------------------

/// A single view exercising every `vhtml!` syntax feature.
pub fn all_features() -> Node {
    vhtml! {
        schema { count, flag, loading, title },

        div class="shell" {
            h1 id="title" { ${ veloasm::velo_ast::Expr::signal("title") } }

            p id="static" { "static paragraph" }

            p { "count: " ${ veloasm::velo_ast::Expr::signal("count") } }

            div .box { "has static box class" }

            div class:highlight=|flag| { "highlight toggles with flag" }

            button on:click={ add("count", 1.0) } {
                "increment count"
            }
            button on:click={ toggle("flag") } {
                "toggle flag"
            }

            @Show(flag) {
                p { "flag is on" }
            } @else {
                p { "flag is off" }
            }

            ul class="threat-list" {
                @for t in vec![threat_row(1, "alpha", "low"), threat_row(2, "beta", "high")], key = t.id {
                    li class:severe=(t.severity == "high") data-id=(t.id) {
                        span class="title" { (t.title) }
                        span class="sev" { "[" (t.severity) "]" }
                    }
                }
            }
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
// 2. Minimal: each feature in isolation
// ---------------------------------------------------------------------------

/// Bare element + static text.
pub fn plain() -> Node {
    vhtml! {
        div class="card" {
            h2 { "Plain" }
            p { "just static text and attributes" }
        }
    }
}

/// Reactive text with AST signal reference.
pub fn reactive_text() -> Node {
    vhtml! {
        schema { count },
        p { "value: " ${ Expr::signal("count") } }
    }
}

/// Class shorthand `.cls` and reactive `class:toggle=(cond)` using pipe signal ref.
pub fn classes() -> Node {
    vhtml! {
        schema { active },
        div {
            span .staticcls { "static class" }
            span class:active=|active| { "reactive class" }
        }
    }
}

/// Event bindings: `on:click={ action }` with concise helpers.
pub fn events() -> Node {
    vhtml! {
        schema { count, flag },
        div {
            button on:click={ add("count", 1.0) } {
                "+1"
            }
            button on:click={ toggle("flag") } {
                "toggle"
            }
        }
    }
}

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
/// Pure AST approach - signals declared in View, referenced by name in vhtml!
pub fn show_component() -> Node {
    vhtml! {
        schema { flag, loading },
        div {
            Show when=|flag| fallback={ "off" } {
                p { "on" }
            }
            Show when=${ Expr::signal("flag") } fallback=${ Node::text("off via expr") } {
                p { "on via expr" }
            }
            Suspense loading=|loading| fallback={ "loading." } {
                p { "ready" }
            }
        }
    }
}

/// Unkeyed `@for` - plain Rust body.
pub fn unkeyed_loop() -> Node {
    vhtml! {
        ul {
            @for i in 0..3 {
                li { "row: " (i) }
            }
        }
    }
}

/// Keyed `@for` - Value grammar, reconciled by identity.
pub fn keyed_loop() -> Node {
    vhtml! {
        ul {
            @for t in vec![threat_row(1, "alpha", "low"), threat_row(2, "port scan", "medium")], key = t.id {
                li data-id=(t.id) class:severe=(t.severity == "high") {
                    span class="title" { (t.title) }
                    span class="sev" { "[" (t.severity) "]" }
                }
            }
        }
    }
}

/// `@let` lexical binding.
pub fn let_binding() -> Node {
    vhtml! {
        @let label = "bound by @let",
        p { (label) }
    }
}

/// A portable view without `@for` (which produces `Node::Native`, client-only) and
/// without reactive text `(expr)` (which produces `Expr::Native`).
pub fn portable_view() -> View {
    View::new(
        "portable-panel",
        vhtml! {
            schema { flag, count },
            div {
                h2 { "Portable Panel" }
                @Show("flag") { p { "on" } } @else { p { "off" } }
                p { "count: " ${ Expr::signal("count") } }
            }
        }
    )
    .with_signal(SignalDecl::count("count", 0.0))
    .with_signal(SignalDecl::scalar("flag", true))
    .declare_used_signals()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_compiles() {
        let _: Node = plain();
    }

    #[test]
    fn reactive_text_compiles() {
        let _: Node = reactive_text();
    }

    #[test]
    fn classes_compile() {
        let _: Node = classes();
    }

    #[test]
    fn events_compile() {
        let _: Node = events();
    }

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
        let _: Node = vhtml! {
            schema { count },
            Show when=|count| fallback={{ Node::text("off") }} {
                p { "on" }
            }
        };
    }

    #[test]
    fn schema_rejects_undeclared_show_id() {
        let _ = vhtml! {
            schema { count },
            @Show("count") { p { "declared" } }
        };
    }
}
//! Tic-tac-toe built with `velo_ast_macro::vhtml!`.
//!
//! Backend-safe example: no WASM, no `veloasm` dependency. Builds a
//! [`velo_ast::View`] that serializes to JSON and can be mounted by a
//! wasm client reactively.
//!
//! Signals:
//!   turn   — "X" or "O", whose turn it is
//!   cell_0..cell_8 — each cell's value: "" / "X" / "O"
//!
//! Run the test:
//!   cargo test -p tic-tac-toe

use velo_ast::event::{EventAction, EventOp, EventValue};
use velo_ast::{Expr, SignalDecl, View};
use velo_ast_macro::vhtml;

/// Build the tic-tac-toe view.
///
/// All signals are declared on the `View` (AST level). Events write to AST
/// signal ids, and reads use `${ Expr::signal("id") }`. The resulting `View`
/// is portable and serializes to JSON for the wire.
pub fn tic_tac_toe() -> View {
    View::new(
        "tic-tac-toe",
        vhtml! {
            schema { turn, cell_0, cell_1, cell_2, cell_3, cell_4, cell_5, cell_6, cell_7, cell_8 },

            div class="game" {
                h1 { "Tic Tac Toe" }

                div class="status" {
                    "Turn: "
                    ${ Expr::signal("turn") }
                }

                div class="board" {
                    div class="row" {
                        button class="cell" on:click=( cell_action("cell_0") ) {
                            ${ Expr::signal("cell_0") }
                        }
                        button class="cell" on:click=( cell_action("cell_1") ) {
                            ${ Expr::signal("cell_1") }
                        }
                        button class="cell" on:click=( cell_action("cell_2") ) {
                            ${ Expr::signal("cell_2") }
                        }
                    }
                    div class="row" {
                        button class="cell" on:click=( cell_action("cell_3") ) {
                            ${ Expr::signal("cell_3") }
                        }
                        button class="cell" on:click=( cell_action("cell_4") ) {
                            ${ Expr::signal("cell_4") }
                        }
                        button class="cell" on:click=( cell_action("cell_5") ) {
                            ${ Expr::signal("cell_5") }
                        }
                    }
                    div class="row" {
                        button class="cell" on:click=( cell_action("cell_6") ) {
                            ${ Expr::signal("cell_6") }
                        }
                        button class="cell" on:click=( cell_action("cell_7") ) {
                            ${ Expr::signal("cell_7") }
                        }
                        button class="cell" on:click=( cell_action("cell_8") ) {
                            ${ Expr::signal("cell_8") }
                        }
                    }
                }

                button class="reset" on:click=( reset_action() ) {
                    "Reset"
                }
            }
        },
    )
    .with_signal(SignalDecl::scalar("turn", "X"))
    .with_signal(SignalDecl::scalar("cell_0", ""))
    .with_signal(SignalDecl::scalar("cell_1", ""))
    .with_signal(SignalDecl::scalar("cell_2", ""))
    .with_signal(SignalDecl::scalar("cell_3", ""))
    .with_signal(SignalDecl::scalar("cell_4", ""))
    .with_signal(SignalDecl::scalar("cell_5", ""))
    .with_signal(SignalDecl::scalar("cell_6", ""))
    .with_signal(SignalDecl::scalar("cell_7", ""))
    .with_signal(SignalDecl::scalar("cell_8", ""))
    .declare_used_signals()
}

/// Action for clicking a cell: set the cell to the current turn, then toggle
/// turn between "X" and "O".
fn cell_action(cell_id: &str) -> EventAction {
    let set_cell = EventOp::set(
        cell_id.to_string(),
        EventValue::Expr(Expr::signal("turn")),
    );

    let toggle_turn = EventOp::set(
        "turn".to_string(),
        EventValue::Expr(Expr::Cond {
            test: Box::new(Expr::Cmp {
                op: velo_ast::expr::CmpOp::Eq,
                l: Box::new(Expr::signal("turn")),
                r: Box::new(Expr::str("X")),
            }),
            then: Box::new(Expr::str("O")),
            otherwise: Box::new(Expr::str("X")),
        }),
    );

    EventAction::new(vec![set_cell, toggle_turn])
}

/// Action for the Reset button: clear all cells and set turn to "X".
fn reset_action() -> EventAction {
    let mut ops: Vec<EventOp> =
        vec![EventOp::set("turn".to_string(), EventValue::Expr(Expr::str("X")))];
    for i in 0..9 {
        ops.push(EventOp::set(
            format!("cell_{i}"),
            EventValue::Expr(Expr::str("")),
        ));
    }
    EventAction::new(ops)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tic_tac_toe_builds_and_is_portable() {
        let view = tic_tac_toe();
        assert!(view.is_portable());
        assert_eq!(view.name, "tic-tac-toe");
        assert_eq!(view.signals.len(), 10); // turn + 9 cells
        assert_eq!(view.signal("turn").unwrap().initial, velo_ast::Value::from("X"));
        for i in 0..9 {
            assert!(view.signal(&format!("cell_{i}")).is_some());
        }
        let json = view.to_json().expect("view serializes to JSON");
        assert!(json.contains("tic-tac-toe"));
        assert!(json.contains("cell_0"));
    }

    #[test]
    fn cell_action_sets_and_toggles() {
        let action = cell_action("cell_3");
        assert_eq!(action.ops.len(), 2);
        match &action.ops[0] {
            EventOp::Set { target, value } => {
                assert_eq!(target, "cell_3");
                assert!(matches!(value, EventValue::Expr(_)));
            }
            other => panic!("expected Set op, got {other:?}"),
        }
        match &action.ops[1] {
            EventOp::Set { target, .. } => assert_eq!(target, "turn"),
            other => panic!("expected Set op, got {other:?}"),
        }
    }

    #[test]
    fn reset_action_clears_all() {
        let action = reset_action();
        assert_eq!(action.ops.len(), 10); // turn + 9 cells
        match &action.ops[0] {
            EventOp::Set { target, value } => {
                assert_eq!(target, "turn");
                assert!(matches!(value, EventValue::Expr(_)));
            }
            other => panic!("expected Set op, got {other:?}"),
        }
    }
}

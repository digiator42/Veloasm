//! Structural tests for the `vhtml!` keyed `@for` lowering.
//!
//! Host-side (no browser): assert the expanded tree is the keyed
//! `Node::Each` the reconciler understands, with reads at the depths the
//! resolver expects. Behavioral reconciliation is exercised by the
//! ast-dashboard browser suite. These live outside the lib target because
//! the macro emits `::veloasm::` paths, which resolve only where the
//! library is a dependency — not inside it.

use veloasm::vhtml;
use veloasm::velo_ast::{Expr, Node};

#[test]
fn unkeyed_for_stays_a_plain_rust_loop() {
    let node = vhtml! {
        @for t in 0..3 {
            p class:row=(t % 2 == 0) { (format!("row {}", t)) }
        }
    };
    assert!(matches!(node, Node::Native(..)), "unkeyed `@for` must not become `Node::Each`");
}

#[test]
fn indexed_key_for_lowers_to_each_with_a_loop_index_key() {
    let node = vhtml! {
        @for (t, i) in 0..3, key = i {
            p { (format!("row {}", t)) }
        }
    };
    let Node::Each { items, key, body } = node else {
        panic!("a keyed `@for` must lower to `Node::Each`, got {node:?}");
    };
    let key = key.expect("the reconciler only applies when a key is present");
    assert!(matches!(key, Expr::LoopIndex(0)), "an index-aliased key reads as depth 0");
    assert!(matches!(items, Expr::Native(..)), "items stay a native list of Rust rows");
    assert!(matches!(*body, Node::Element(..)), "the row body is one template element");
}

#[test]
fn keyed_for_reads_the_row_through_member_exprs() {
    let threats = veloasm::signal!(vec![0i32]);
    let node = vhtml! {
        @for t in threats.get(), key = t.id {
            li class:hot=(t.severity == "high") { (format!("{} — {}", t.title, t.severity)) }
        }
    };
    let Node::Each { key, body, .. } = node else {
        panic!("a keyed `@for` must lower to `Node::Each`, got {node:?}");
    };
    let key = key.expect("the reconciler only applies when a key is present");
    let Expr::Member { base, key: field } = &key else {
        panic!("`key = t.id` must lower to a member read over the loop item, got {key:?}");
    };
    assert!(matches!(**base, Expr::LoopItem(0)), "the row of this loop reads at depth 0");
    assert_eq!(field, "id");

    let Node::Element(el) = *body else { panic!("body must be an element") };
    let Node::Text(read) = el.children[0].clone() else {
        panic!("the splice must lower to a reactive text read of the row");
    };
    let Expr::Format { parts, .. } = read else {
        panic!("`format!` must lower to a template over member reads, got {read:?}");
    };
    for part in &parts {
        match part {
            Expr::Str(_) => {}
            Expr::Member { base, .. } => {
                assert!(matches!(**base, Expr::LoopItem(0)), "row reads stay at depth 0");
            }
            _ => panic!("`format!` parts must be literals or row reads"),
        }
    }
    let _ = threats;
}

#[test]
fn nested_keyed_loops_shift_the_outer_depth() {
    let outer = veloasm::signal!(vec![0i32]);
    let node = vhtml! {
        @for t in outer.get(), key = t.id {
            @for r in t.children, key = r.code {
                div { (t.title) (r.name) }
            }
        }
    };
    let Node::Each { body, .. } = node else { panic!("outer loop must be `Node::Each`") };
    let Node::Each { key, body: inner_body, .. } = *body else {
        panic!("inner loop must also be `Node::Each`");
    };
    let key = key.expect("the reconciler only applies when a key is present");
    let Expr::Member { base, .. } = &key else { panic!("`key = r.code` must be a member read") };
    assert!(matches!(**base, Expr::LoopItem(0)), "the inner row reads at depth 0");

    let Node::Element(el) = *inner_body else { panic!("inner body must be an element") };
    let Node::Fragment(kids) = &el.children[0] else {
        panic!("splices must lower to two reactive texts, children={:?}", el.children);
    };
    let [Node::Text(a), Node::Text(b)] = kids.as_slice() else {
        panic!("two splices, got {:?}", kids)
    };
    let Expr::Member { base: a_base, .. } = a else { panic!("outer read is a member read") };
    assert!(matches!(**a_base, Expr::LoopItem(1)), "the outer row is one level out");
    let Expr::Member { base: b_base, .. } = b else { panic!("inner read is a member read") };
    assert!(matches!(**b_base, Expr::LoopItem(0)), "the inner row reads at depth 0");
    let _ = outer;
}

//! The browser client, end to end — with the tests running *in* the browser.
//!
//! # What this proves
//!
//! The whole point of the shared AST is that a view built on a backend behaves
//! the same in a browser: reactive text, class toggles, two-way bindings,
//! keyed lists, and event handlers — all without a page reload.
//!
//! Rather than assert that from the outside, the app asserts from the inside:
//! [`run_self_tests`] dispatches **real DOM events** against the mounted view
//! and inspects the resulting DOM, then writes the results into the page. A
//! headless browser can therefore read the verdict straight out of the markup.
//!
//! ```text
//! $ trunk serve            # or: cargo run --bin ast-dashboard -- --emit-view
//! $ chrome --headless --virtual-time-budget=8000 --dump-dom http://localhost:8080
//! ...
//! <pre id="test-results">PASS mount-renders-the-view
//! PASS reactive-text-updates-on-signal-change
//! ...</pre>
//! ```
//!
//! # The view
//!
//! One view, built by [`demo_view`], exercises every feature the client
//! supports. The host binary serialises it to `view.json`; the browser fetches
//! that file and mounts it — so the test runs against the **wire format**, not
//! against a view built in the browser.

use std::collections::BTreeMap;

use veloasm::velo_ast::attr::Attr;
use veloasm::velo_ast::event::{EventAction, EventOp};
use veloasm::velo_ast::{
    Expr, HandlerRef, Node, ServerRoute, SignalDecl, Value, View,
};
use veloasm::vhtml;

// The browser-side helpers talk to the DOM directly, so they need the
// wasm-bindgen casts and the JS value type.
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

// =============================================================================
// The demo view — one view, every client feature
// =============================================================================

/// The view the browser mounts.
///
/// It is deliberately built with the **builder** API rather than `vhtml!`:
/// this is the shape a backend produces, so it is the shape that has to prove
/// itself over the wire.
pub fn demo_view() -> View {
    View::new(
        "ast-dashboard",
        Node::element("section")
            .static_attr("id", "panel")
            .static_attr("class", "panel")
            // A reactive class toggle, folded with the static base by the
            // renderer so the two writers of `class` never clobber each other.
            .attr(Attr::class_toggle("is-on", Expr::signal("flag")))
            .child(Node::element("span").static_attr("id", "count-value").child(
                Node::text_expr(Expr::signal("count")),
            ))
            .child(Node::element("span").static_attr("id", "flag-value").child(
                Node::text_expr(Expr::signal("flag")),
            ))
            .child(
                Node::element("input")
                    .static_attr("id", "draft")
                    .static_attr("type", "text")
                    .attr(Attr::bind_value(Expr::signal("draft"))),
            )
            .child(Node::element("span").static_attr("id", "draft-value").child(
                Node::text_expr(Expr::signal("draft")),
            ))
            // A keyed list: rows are reconciled by identity, so an append costs
            // one node rather than the whole table.
            .child(
                Node::element("ul")
                    .static_attr("id", "rows")
                    .child(Node::each(
                        Expr::signal("rows"),
                        Expr::item_field(0, "id"),
                        Node::element("li")
                            .static_attr("class", "row")
                            .child(Node::element("span").static_attr("class", "row-label").child(
                                Node::text_expr(Expr::item_field(0, "label")),
                            )),
                    )),
            )
            .child(
                Node::element("button")
                    .static_attr("id", "inc")
                    .on(
                        "click",
                        EventAction::one(EventOp::Update {
                            target: "count".into(),
                            op: veloasm::velo_ast::ArithOp::Add,
                            by: veloasm::velo_ast::EventValue::Expr(Expr::Num(1.0)),
                        }),
                    )
                    .child(Node::text("increment")),
            )
            .child(
                Node::element("button")
                    .static_attr("id", "toggle")
                    .on(
                        "click",
                        EventAction::one(EventOp::Toggle {
                            target: "flag".into(),
                        }),
                    )
                    .child(Node::text("toggle")),
            )
            .child(
                Node::element("button")
                    .static_attr("id", "set-draft")
                    .on(
                        "click",
                        EventAction::one(EventOp::Set {
                            target: "draft".into(),
                            value: veloasm::velo_ast::EventValue::Expr(Expr::str("from-signal")),
                        }),
                    )
                    .child(Node::text("set draft")),
            )
            .child(
                Node::element("button")
                    .static_attr("id", "add-row")
                    .on(
                        "click",
                        EventAction::one(EventOp::Push {
                            target: "rows".into(),
                            value: veloasm::velo_ast::EventValue::Expr(Expr::Map(vec![
                                ("id".into(), Expr::Num(99.0)),
                                ("label".into(), Expr::str("appended")),
                            ])),
                        }),
                    )
                    .child(Node::text("add row")),
            )
            // A handler that lives on the *client* — the one thing a server
            // route cannot do.
            .child(
                Node::element("button")
                    .static_attr("id", "local")
                    .on(
                        "click",
                        EventAction::one(EventOp::Call {
                            handler: "mark_seen".into(),
                        }),
                    )
                    .child(Node::text("local")),
            )
            .child(Node::element("span").static_attr("id", "seen-value").child(
                Node::text_expr(Expr::signal("seen")),
            )),
    )
    .with_signal(SignalDecl::count("count", 1.0))
    .with_signal(SignalDecl::scalar("flag", false))
    .with_signal(SignalDecl::scalar("draft", ""))
    .with_signal(SignalDecl::scalar("seen", "no"))
    .with_signal(SignalDecl::list("rows", sample_rows()).capped(50))
    // The server route is *declared* here but never called by the tests: it
    // would need a live server. Declaring it proves the handler table
    // travels with the view.
    .with_handler(HandlerRef::remote(
        "reload",
        ServerRoute::post("/api/rows/reload"),
        "rows",
    ))
}

fn sample_rows() -> Vec<Value> {
    vec![
        Value::map([
            ("id", Value::Num(1.0)),
            ("label", Value::Str("first".into())),
        ]),
        Value::map([
            ("id", Value::Num(2.0)),
            ("label", Value::Str("second".into())),
        ]),
    ]
}

/// A second view, written in Maud's syntax, to show the macro path produces the
/// same kind of tree.
pub fn maud_style_view() -> Node {
    let heading = "Sessions";
    vhtml! {
        section class="panel" {
            h2 { (heading) }
            @for who in ["ada", "bob"] {
                li { who }
            }
            input type="text" placeholder="Filter" disabled;
            Show when="loading" {
                span { "Loading..." }
            }
        }
    }
}

// =============================================================================
// The in-browser test harness
// =============================================================================

/// One assertion's verdict.
pub struct TestResult {
    /// What was being asserted.
    pub name: String,
    /// Whether it held.
    pub passed: bool,
    /// Why it failed, when it did.
    pub detail: String,
}

impl TestResult {
    /// Only the wasm self-test runner constructs verdicts.
    #[cfg(target_arch = "wasm32")]
    fn pass(name: &str) -> Self {
        TestResult {
            name: name.to_string(),
            passed: true,
            detail: String::new(),
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn fail(name: &str, detail: impl Into<String>) -> Self {
        TestResult {
            name: name.to_string(),
            passed: false,
            detail: detail.into(),
        }
    }
}

/// Run every browser-side assertion and return the verdicts.
///
/// Only meaningful in a browser — it drives the real DOM.
#[cfg(target_arch = "wasm32")]
pub fn run_self_tests() -> Vec<TestResult> {

    let mut results = Vec::new();

    let doc = veloasm::document();
    let panel = doc.get_element_by_id("panel");

    // 1. The view mounted.
    results.push(match &panel {
        Some(_) => TestResult::pass("mount-renders-the-view"),
        None => TestResult::fail("mount-renders-the-view", "no #panel in the DOM"),
    });

    if panel.is_none() {
        return results;
    }

    let text_of = |id: &str| -> Option<String> {
        doc.get_element_by_id(id).map(|e| e.text_content().unwrap_or_default())
    };
    let class_of = |id: &str| -> Option<String> {
        doc.get_element_by_id(id).map(|e| e.get_attribute("class").unwrap_or_default())
    };
    let rows = |()| -> Vec<web_sys::Element> {
        match doc.get_element_by_id("rows") {
            Some(ul) => ul
                .query_selector_all("li")
                .map(|list| {
                    (0..list.length())
                        .filter_map(|i| list.item(i).and_then(|n| n.dyn_into().ok()))
                        .collect()
                })
                .unwrap_or_default(),
            None => Vec::new(),
        }
    };

    // 2. The seed values arrived from the wire.
    results.push(match text_of("count-value") {
        Some(v) if v.trim() == "1" => TestResult::pass("seed-values-arrive-from-the-wire"),
        other => TestResult::fail(
            "seed-values-arrive-from-the-wire",
            format!("count-value was {other:?}, expected \"1\""),
        ),
    });

    // 3. The keyed list rendered one row per record.
    results.push(match rows(()).len() {
        2 => TestResult::pass("keyed-list-renders-every-row"),
        n => TestResult::fail(
            "keyed-list-renders-every-row",
            format!("{n} rows, expected 2"),
        ),
    });

    // 4. A click updates a reactive text node.
    click(&doc, "inc");
    results.push(match text_of("count-value") {
        Some(v) if v.trim() == "2" => TestResult::pass("reactive-text-updates-on-signal-change"),
        other => TestResult::fail(
            "reactive-text-updates-on-signal-change",
            format!("count-value was {other:?}, expected \"2\""),
        ),
    });

    // 5. A class toggle flips, without losing the base class.
    click(&doc, "toggle");
    results.push(match class_of("panel") {
        Some(c) if c.contains("is-on") && c.contains("panel") => {
            TestResult::pass("class-toggle-adds-without-clobbering-the-base")
        }
        other => TestResult::fail(
            "class-toggle-adds-without-clobbering-the-base",
            format!("panel class was {other:?}"),
        ),
    });

    // 6. A signal writes into the DOM (signal -> DOM).
    click(&doc, "set-draft");
    results.push(match doc.get_element_by_id("draft") {
        Some(input) => {
            let value = input
                .dyn_into::<web_sys::HtmlInputElement>()
                .map(|i| i.value())
                .unwrap_or_default();
            if value == "from-signal" {
                TestResult::pass("signal-writes-into-the-dom")
            } else {
                TestResult::fail(
                    "signal-writes-into-the-dom",
                    format!("input value was {value:?}"),
                )
            }
        }
        None => TestResult::fail("signal-writes-into-the-dom", "no #draft"),
    });

    // 7. Typing writes into the signal (DOM -> signal).
    if let Some(input) = doc.get_element_by_id("draft") {
        if let Ok(el) = input.clone().dyn_into::<web_sys::HtmlInputElement>() {
            let _ = el.set_value("typed");
        }
        if let Ok(event) = web_sys::Event::new("input") {
            let _ = input.dispatch_event(&event);
        }
    }
    results.push(match text_of("draft-value") {
        Some(v) if v.trim() == "typed" => TestResult::pass("dom-writes-into-the-signal"),
        other => TestResult::fail(
            "dom-writes-into-the-signal",
            format!("draft-value was {other:?}, expected \"typed\""),
        ),
    });

    // 8. Appending a row adds exactly one row...
    click(&doc, "add-row");
    results.push(match rows(()).len() {
        3 => TestResult::pass("append-adds-exactly-one-row"),
        n => TestResult::fail("append-adds-exactly-one-row", format!("{n} rows, expected 3")),
    });

    // 9. ...and reuses the existing rows' DOM nodes rather than rebuilding
    //    them. The stamp survives only if the very same element is still there.
    let stamped = doc
        .get_element_by_id("rows")
        .and_then(|ul| ul.query_selector("li").ok().flatten())
        .map(|li| {
            js_sys::Reflect::set(&li, &"__stamp".into(), &"original".into()).ok();
            true
        })
        .unwrap_or(false);
    click(&doc, "add-row");
    let stamp_survived = doc
        .get_element_by_id("rows")
        .and_then(|ul| ul.query_selector("li").ok().flatten())
        .and_then(|li| js_sys::Reflect::get(&li, &"__stamp".into()).ok())
        .map(|v| v.as_string() == Some("original".into()))
        .unwrap_or(false);
    results.push(if stamped && stamp_survived {
        TestResult::pass("keyed-list-reuses-existing-row-nodes")
    } else {
        TestResult::fail(
            "keyed-list-reuses-existing-row-nodes",
            "the first row's DOM node was replaced instead of reused",
        )
    });

    // 10. A client-local handler runs.
    click(&doc, "local");
    results.push(match text_of("seen-value") {
        Some(v) if v.trim() == "yes" => TestResult::pass("client-local-handler-runs"),
        other => TestResult::fail(
            "client-local-handler-runs",
            format!("seen-value was {other:?}, expected \"yes\""),
        ),
    });

    results.extend(vhtml_loop_tests(&doc));
    results
}

/// Assertions for the `vhtml!` island — the Maud path.
///
/// These are the tests that decide whether the feature is worth anything: the
/// list has to update from a signal change with no re-render, written in plain
/// Rust rather than AST nodes.
#[cfg(target_arch = "wasm32")]
fn vhtml_loop_tests(doc: &web_sys::Document) -> Vec<TestResult> {
    let mut results = Vec::new();

    let rows = |()| -> Vec<web_sys::Element> {
        match doc.get_element_by_id("vhtml-rows") {
            Some(ul) => ul
                .query_selector_all("li")
                .map(|list| {
                    (0..list.length())
                        .filter_map(|i| list.item(i).and_then(|n| n.dyn_into().ok()))
                        .collect()
                })
                .unwrap_or_default(),
            None => Vec::new(),
        }
    };

    // 11. The `@for` loop rendered the rows it was handed.
    let initial = rows(());
    results.push(match (initial.len(), initial.first()) {
        (1, Some(li)) if li.text_content().unwrap_or_default().trim() == "brute force" => {
            TestResult::pass("vhtml-for-renders-its-rows")
        }
        (n, _) => TestResult::fail(
            "vhtml-for-renders-its-rows",
            format!("{n} rows, expected 1 reading \"brute force\""),
        ),
    });

    // 12. The Rust-computed class toggle landed on the row.
    results.push(match initial.first() {
        Some(li) if li.get_attribute("class").unwrap_or_default().contains("critical") => {
            TestResult::pass("vhtml-class-toggle-computed-in-rust")
        }
        Some(li) => TestResult::fail(
            "vhtml-class-toggle-computed-in-rust",
            format!("row class was {:?}", li.get_attribute("class")),
        ),
        None => TestResult::fail("vhtml-class-toggle-computed-in-rust", "no row to check"),
    });

    // 13. Writing the signal re-runs the loop — the whole point. Nothing else
    //     re-rendered the view; only the list's signal moved.
    click(doc, "vhtml-add");
    let after_add = rows(());
    results.push(match after_add.len() {
        2 => TestResult::pass("vhtml-for-reacts-to-a-signal-change"),
        n => TestResult::fail(
            "vhtml-for-reacts-to-a-signal-change",
            format!("{n} rows after adding one, expected 2"),
        ),
    });

    // 13b. The keyed island in the same loop tracks the same signal.
    let keyed = |()| -> Vec<web_sys::Element> {
        match doc.get_element_by_id("vhtml-keyed") {
            Some(ul) => ul
                .query_selector_all("li")
                .map(|list| {
                    (0..list.length())
                        .filter_map(|i| list.item(i).and_then(|n| n.dyn_into().ok()))
                        .collect()
                })
                .unwrap_or_default(),
            None => Vec::new(),
        }
    };
    let keyed_after_add = keyed(());
    results.push(match keyed_after_add.len() {
        2 => TestResult::pass("keyed-for-tracks-the-same-signal"),
        n => TestResult::fail(
            "keyed-for-tracks-the-same-signal",
            format!("{n} keyed rows after adding one, expected 2"),
        ),
    });

    // 14. A row's toggle is recomputed for the new row, not inherited.
    results.push(match after_add.last() {
        Some(li) => {
            let class = li.get_attribute("class").unwrap_or_default();
            if class.contains("critical") {
                TestResult::fail(
                    "vhtml-rows-recompute-their-own-toggles",
                    "the low-severity row was marked critical",
                )
            } else {
                TestResult::pass("vhtml-rows-recompute-their-own-toggles")
            }
        }
        None => TestResult::fail("vhtml-rows-recompute-their-own-toggles", "no row to check"),
    });

    // 15. Keyed rows keep identity across reordering: after moving the first
    //     row to the end, the data-ids follow the data, not the positions.
    click(doc, "vhtml-shuffle");
    let keyed_ids = keyed(())
        .iter()
        .filter_map(|li| li.get_attribute("data-id"))
        .collect::<Vec<String>>();
    let keyed_order: Vec<&str> = keyed_ids.iter().map(|id| id.as_str()).collect();
    results.push(match keyed_order.as_slice() {
        ["2", "1"] => TestResult::pass("keyed-for-reorders-doms-with-identity"),
        order => TestResult::fail(
            "keyed-for-reorders-doms-with-identity",
            format!("key order after shuffle was {order:?}, expected [\"2\", \"1\"]"),
        ),
    });

    // 16. Emptying the list removes every row.
    click(doc, "vhtml-clear");
    let cleared = (rows(()).len(), keyed(()).len());
    results.push(match cleared {
        (0, 0) => TestResult::pass("vhtml-for-renders-nothing-for-an-empty-list"),
        (a, b) => TestResult::fail(
            "vhtml-for-renders-nothing-for-an-empty-list",
            format!("after clearing: {a} plain rows, {b} keyed rows, expected 0 rows"),
        ),
    });

    results
}

/// Dispatch a real `click` on the element with `id`.
#[cfg(target_arch = "wasm32")]
fn click(doc: &web_sys::Document, id: &str) {
    let Some(element) = doc.get_element_by_id(id) else {
        return;
    };
    if let Ok(event) = web_sys::MouseEvent::new("click") {
        let _ = element.dispatch_event(&event);
    }
}

/// The browser entry point: fetch the view, mount it, run the tests.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    wasm_bindgen_futures::spawn_local(async {
        let outcome = match veloasm::fetch_json::<View>("/view.json").await {
            Ok(view) => {
                // A `Runtime` rather than `ast::mount`, so a client-local
                // handler can be registered before the view is rendered. This
                // is the one piece of behaviour that cannot live on the server.
                let runtime = veloasm::ast::Runtime::new(&view);
                runtime.on("mark_seen", |ctx| {
                    if let Some(seen) = ctx.store.signal("seen") {
                        seen.set(Value::Str("yes".into()));
                    }
                });
                let node = veloasm::ast::render(&view, &runtime);
                veloasm::mount(node);
                mount_vhtml_island(&doc_for_mount());
                Some(run_self_tests())
            }
            Err(e) => {
                web_sys::console::error_1(&JsValue::from_str(&format!("velo: could not load /view.json: {e}")));
                None
            }
        };
        write_results(outcome.unwrap_or_default());
    });
}

/// Write the verdicts into the page so a headless browser can read them.
#[cfg(target_arch = "wasm32")]
fn write_results(results: Vec<TestResult>) {
    let doc = veloasm::document();
    let el = doc
        .get_element_by_id("test-results")
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        .unwrap_or_else(|| {
            let el = doc
                .create_element("pre")
                .expect("create <pre>")
                .dyn_into::<web_sys::HtmlElement>()
                .expect("<pre> is an HtmlElement");
            el.set_id("test-results");
            el
        });
    let text = results
        .iter()
        .map(|r| {
            if r.passed {
                format!("PASS {}", r.name)
            } else {
                format!("FAIL {}: {}", r.name, r.detail)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    el.set_inner_html("");
    let _ = el.set_text_content(Some(&text));
}

// =============================================================================
// The Maud path: vhtml!
// =============================================================================

/// A row in the hand-written list. A plain domain type — no `Value`, no
/// serialization, no AST ceremony. This is the type a Maud developer already
/// has.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Debug, PartialEq)]
pub struct Threat {
    /// Stable identity.
    pub id: u32,
    /// Human label.
    pub title: String,
    /// Drives the row's class toggle.
    pub severity: &'static str,
}

#[cfg(target_arch = "wasm32")]
impl Threat {
    /// A sample row.
    fn new(id: u32, title: &str, severity: &'static str) -> Self {
        Threat {
            id,
            title: title.to_string(),
            severity,
        }
    }
}

/// Mount the hand-written `vhtml!` island into `#vhtml-panel`.
///
/// This is the point of the whole feature, so it is deliberately written the
/// way a Maud developer would write it: a signal holding a `Vec<Threat>`, a
/// `@for` over it, field access in the body, and a class toggle computed in
/// Rust. No AST types appear below this line.
#[cfg(target_arch = "wasm32")]
fn mount_vhtml_island(doc: &web_sys::Document) {
    use veloasm::{signal, vhtml};

    // The list itself is a signal, so writing to it re-runs the loop.
    let threats = signal!(vec![Threat::new(1, "brute force", "high")]);

    // The keyed island mirrors the same data through `Value` rows: keyed
    // reconciliation reads fields from values, so the loop's source list has
    // to be convertible — the price of the key, next to the row-grammar body.
    let threats_to_rows = |rows: &[Threat]| -> Vec<veloasm::velo_ast::Value> {
        rows.iter()
            .map(|t| {
                veloasm::velo_ast::Value::map([
                    ("id", veloasm::velo_ast::Value::from(t.id as usize)),
                    ("title", veloasm::velo_ast::Value::from(t.title.as_str())),
                    ("severity", veloasm::velo_ast::Value::from(t.severity)),
                ])
            })
            .collect()
    };
    let keyed_rows = signal!(threats_to_rows(&threats.get()));

    let root = vhtml! {
        section id="vhtml-list" {
            ul id="vhtml-rows" {
                @for t in threats.get() {
                    li class:critical=(t.severity == "high") { (t.title) }
                }
            }
            ul id="vhtml-keyed" {
                @for t in keyed_rows.get(), key = t.id {
                    li data-id=(format!("{}", t.id)) { (t.title) }
                }
            }
            button id="vhtml-add" { "add threat" }
            button id="vhtml-shuffle" { "shuffle" }
            button id="vhtml-clear" { "clear" }
        }
    };

    // The buttons are plain DOM: this island is about the *view*, and a
    // `vhtml!` event binding needs a server handler, which is the other half.
    // Writing the signal is all it takes to make the loop re-run.
    if let Some(host) = doc.get_element_by_id("vhtml-panel") {
        let node = veloasm::ast::mount_node(root);
        if let Some(host) = host.dyn_into::<web_sys::Element>().ok() {
            let _ = host.append_child(&node.raw_node);
        }
        veloasm::mount(node);
    }

    let threats_add = threats;
    let threats_shuffle = threats;
    let threats_clear = threats;
    let keyed_add = keyed_rows;
    let keyed_shuffle = keyed_rows;
    let keyed_clear = keyed_rows;
    on_click(doc, "vhtml-add", move || {
        let mut next = threats_add.get();
        next.push(Threat::new(next.len() as u32 + 1, "port scan", "low"));
        threats_add.set(next);
        keyed_add.set(threats_to_rows(&threats_add.get()));
    });
    // Move the first row to the end: the one structural case keyed
    // reconciliation exists for, and the one that hurts positionally keyed
    // loops — every row but one changes index.
    on_click(doc, "vhtml-shuffle", move || {
        let mut next = threats_shuffle.get();
        if !next.is_empty() {
            let first = next.remove(0);
            next.push(first);
            threats_shuffle.set(next);
            keyed_shuffle.set(threats_to_rows(&threats_shuffle.get()));
        }
    });
    on_click(doc, "vhtml-clear", move || {
        threats_clear.set(Vec::new());
        keyed_clear.set(Vec::new());
    });
}

/// Attach a click listener to `id`.
#[cfg(target_arch = "wasm32")]
fn on_click(doc: &web_sys::Document, id: &str, mut f: impl FnMut() + 'static) {
    let Some(el) = doc.get_element_by_id(id) else {
        return;
    };
    let closure = Closure::wrap(Box::new(move |_: web_sys::MouseEvent| f()) as Box<dyn FnMut(_)>);
    el.add_event_listener_with_callback("click", closure.as_ref().unchecked_ref())
        .expect("addEventListener");
    closure.forget();
}

#[cfg(target_arch = "wasm32")]
fn doc_for_mount() -> web_sys::Document {
    veloasm::document()
}

// =============================================================================
// Host side
// =============================================================================

/// Serialise the demo view to JSON — what a backend ships.
pub fn view_json() -> String {
    demo_view()
        .to_json()
        .expect("the demo view is portable, so it always serializes")
}

/// The SSR markup for the demo view — what a backend serves for the first paint.
pub fn view_html() -> String {
    let view = demo_view();
    let opts = veloasm::velo_ast::HtmlOptions::new();
    let mut out = veloasm::velo_ast::render_with(&view, &opts);
    out.push_str(&veloasm::velo_ast::render_seed_island(&view, &opts));
    out
}

/// A payload map, used by the host binary to report what it emitted.
pub fn summary() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let view = demo_view();
    out.insert("name".into(), view.name.clone());
    out.insert("signals".into(), view.signals.len().to_string());
    out.insert("handlers".into(), view.handlers.len().to_string());
    out.insert("portable".into(), view.is_portable().to_string());
    out.insert("json_bytes".into(), view_json().len().to_string());
    out
}

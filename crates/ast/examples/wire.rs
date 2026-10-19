//! Print the JSON a backend would ship, plus the HTML it would render.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p velo_ast --features serde --example wire
//! ```

use velo_ast::attr::Attr;
use velo_ast::event::{EventAction, EventOp, EventValue};
use velo_ast::path::ObjPath;
use velo_ast::{Expr, HandlerRef, HtmlOptions, Node, ServerRoute, SignalDecl, SignalKind, View};

/// A miniature version of what a backend builds for a dashboard panel.
fn threat_panel() -> View {
    View::new(
        "threat-panel",
        Node::element("section")
            .static_attr("class", "panel")
            .attr(Attr::class_toggle("stale", Expr::signal("refreshing")))
            .attr(Attr::bound(
                "data-count",
                Expr::item(0),
            ))
            .child(Node::element("h2").child(Node::text("Live threats")))
            .child(Node::show(
                Expr::signal("refreshing"),
                Node::text("Refreshing…"),
                Node::each(
                    Expr::signal_at("threats", ObjPath::key("items")),
                    Expr::item_field(0, "id"),
                    Node::element("article")
                        .attr(Attr::class_toggle(
                            "critical",
                            Expr::item_field(0, "critical"),
                        ))
                        .child(Node::text_expr(Expr::item_field(0, "title")))
                        .on(
                            "click",
                            EventAction::new(vec![
                                EventOp::Toggle { target: "selected".into() },
                                EventOp::Call { handler: "acknowledge".into() },
                            ]),
                        ),
                ),
            ))
            .child(
                Node::element("button")
                    .static_attr("type", "button")
                    .on(
                        "click",
                        EventAction::one(EventOp::Update {
                            target: "refreshing".into(),
                            op: velo_ast::ArithOp::Add,
                            by: EventValue::Expr(Expr::Num(1.0)),
                        }),
                    )
                    .child(Node::text("Refresh")),
            ),
    )
    .with_signal(SignalDecl::scalar("refreshing", false))
    .with_signal(SignalDecl::scalar("selected", false))
    .with_signal(SignalDecl::list("threats", Vec::<velo_ast::Value>::new()).capped(500))
    .with_signal(SignalDecl::new("meta", SignalKind::Map))
    // Server-backed: the backend owns this logic, the browser only renders the
    // reply. Nothing about it has to be compiled to WASM.
    .with_handler(HandlerRef::remote(
        "acknowledge",
        ServerRoute::post("/api/v1/threats/{id}/ack"),
        "threats",
    ))
    .with_payload("csrf", "tok-abc123")
}

fn main() {
    let view = threat_panel();

    println!("=== portability ===");
    println!(
        "portable: {}  errors: {:?}",
        view.is_portable(),
        view.portability_errors()
    );
    println!("undeclared signals:  {:?}", view.undeclared_reads());
    println!("undeclared handlers: {:?}", view.undeclared_handlers());

    println!("\n=== SSR (server render, first paint) ===");
    println!("{}", velo_ast::render(&view));

    println!("\n=== SSR with request state ===");
    let opts = HtmlOptions::new()
        .with_signal("refreshing", false)
        .with_signal(
            "threats",
            velo_ast::Value::List(vec![velo_ast::Value::map([
                ("id", velo_ast::Value::Num(42.0)),
                ("title", velo_ast::Value::Str("Brute force on ssh".into())),
                ("critical", velo_ast::Value::Bool(true)),
            ])]),
        )
        .with_path("/threats");
    println!("{}", velo_ast::render_with(&view, &opts));

    println!("\n=== seed island (what the client adopts) ===");
    println!("{}", velo_ast::render_seed_island(&view, &opts));

    let json = view.to_json().expect("serialize");
    println!("=== wire JSON ({} bytes) ===", json.len());
    println!("{}", view.to_json_pretty().unwrap());

    println!("\n=== round trip ===");
    match View::from_json(&json) {
        Ok(back) => println!("identical after JSON round trip: {}", back == view),
        Err(e) => println!("FAILED: {e}"),
    }
}
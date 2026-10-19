//! Wire-format tests: the JSON contract between a backend and a client.
//!
//! These live in a separate module (gated on the `serde` feature) because they
//! are the tests that must keep passing across `AST_VERSION` bumps. Everything
//! else in the crate tests behaviour that is independent of encoding.

use crate::attr::Attr;
use crate::event::{EventAction, EventOp, EventValue, ServerRoute};
use crate::expr::{ArithOp, Expr};
use crate::path::ObjPath;
use crate::node::Node;
use crate::value::Value;
use crate::view::{HandlerRef, SignalDecl, SignalKind, View};

/// A view that exercises every portable construct the AST has.
fn kitchen_sink() -> View {
    View::new("kitchen-sink", {
        Node::element("section")
            .static_attr("id", "root")
            .attr(Attr::boolean("hidden", Expr::Bool(false)))
            .attr(Attr::class_toggle("active", Expr::signal("active")))
            .attr(Attr::style_prop("color", Expr::signal_at("theme", ObjPath::key("fg"))))
            .attr(Attr::bind_value(Expr::signal("draft")))
            .attr(Attr::bind_checked(Expr::signal("agreed")))
            .attr(Attr::reference("editor"))
            .attr(Attr::bound("title", Expr::cond(
                Expr::signal("count"),
                Expr::str("busy"),
                Expr::Null,
            )))
            .on(
                "click",
                EventAction::new(vec![
                    EventOp::Update {
                        target: "count".into(),
                        op: ArithOp::Add,
                        by: EventValue::Expr(Expr::Num(1.0)),
                    },
                    EventOp::Toggle { target: "active".into() },
                    EventOp::Set {
                        target: "draft".into(),
                        value: EventValue::TargetProperty("value".into()),
                    },
                    EventOp::Debounce {
                        ms: 250,
                        ops: vec![EventOp::call("search")],
                    },
                ]),
            )
            .on("submit", EventAction::one(EventOp::Call { handler: "save".into() }))
            .child(Node::text("count: "))
            .child(Node::text_expr(Expr::signal("count")))
            .child(Node::show(
                Expr::signal("agreed"),
                Node::text("thanks"),
                Node::text("please accept"),
            ))
            .child(Node::each(
                Expr::signal_at("threats", ObjPath::parse("items").unwrap()),
                Expr::item_field(0, "id"),
                Node::element("li")
                    .child(Node::text_expr(Expr::item_field(0, "name")))
                    .child(Node::text_expr(Expr::item_field(0, "severity"))),
            ))
            .child(Node::Component {
                name: "ThreatPanel".into(),
                attrs: vec![Attr::bound("rows", Expr::signal("threats"))],
                children: vec![Node::Slot { name: "footer".into() }],
            })
    })
    .with_signal(SignalDecl::count("count", 0.0))
    .with_signal(SignalDecl::new("active", SignalKind::Scalar))
    .with_signal(SignalDecl::list("threats", Vec::<Value>::new()).capped(200))
    .with_signal(SignalDecl::scalar("draft", ""))
    .with_signal(SignalDecl::scalar("agreed", false))
    .with_signal(SignalDecl::new("theme", SignalKind::Map))
    .with_handler(HandlerRef::local("search"))
    .with_handler(HandlerRef::local("save"))
    .with_handler(HandlerRef::remote(
        "disable",
        ServerRoute::post("/api/v1/rules/42/disable").body(Expr::str("{}")),
        "rule",
    ))
    .with_payload("csrf", "tok-123")
    .with_payload(
        "user",
        Value::map([("name", Value::Str("Ada".into())), ("admin", Value::Bool(true))]),
    )
}

#[test]
fn kitchen_sink_view_is_portable() {
    let view = kitchen_sink();
    assert!(view.is_portable(), "portability errors: {:?}", view.portability_errors());
    assert!(view.undeclared_reads().is_empty(), "{:?}", view.undeclared_reads());
    assert!(view.undeclared_handlers().is_empty(), "{:?}", view.undeclared_handlers());
}

#[test]
fn json_round_trip_is_lossless() {
    let view = kitchen_sink();
    let json = view.to_json().expect("serialize");
    let back = View::from_json(&json).expect("deserialize");
    assert_eq!(back, view, "wire round trip lost information");
}

#[test]
fn json_output_is_deterministic() {
    let view = kitchen_sink();
    // Two serializations of the same value must be byte-identical: the payload
    // is a cache key and a test fixture, so map iteration order would be a bug.
    assert_eq!(view.to_json().unwrap(), view.to_json().unwrap());
}

#[test]
fn payload_carries_the_view_version() {
    let json = kitchen_sink().to_json().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["version"], serde_json::json!(crate::AST_VERSION));
    assert_eq!(parsed["name"], "kitchen-sink");
    // Nodes and expressions are adjacently tagged (`n`/`e`), so a payload can be
    // read structurally without knowing the variant names up front. A struct
    // variant's fields sit directly under the content key.
    assert_eq!(parsed["root"]["n"], "Element");
    assert_eq!(parsed["root"]["v"]["tag"], "section");
    let text_node = &parsed["root"]["v"]["children"][1];
    assert_eq!(text_node["n"], "Text");
    assert_eq!(text_node["v"]["e"], "Signal");
    assert_eq!(text_node["v"]["v"]["id"], "count");
}

#[test]
fn a_future_revision_is_rejected_rather_than_mis_rendered() {
    let mut view = kitchen_sink();
    view.version = crate::AST_VERSION + 1;
    let json = view.to_json().unwrap();
    match View::from_json(&json) {
        Err(crate::ViewError::VersionMismatch { found, expected }) => {
            assert_eq!(found, crate::AST_VERSION + 1);
            assert_eq!(expected, crate::AST_VERSION);
        }
        other => panic!("expected a version mismatch, got {other:?}"),
    }
}

#[test]
fn malformed_json_reports_a_decode_error() {
    let err = View::from_json("{not json").unwrap_err();
    assert!(matches!(err, crate::ViewError::Decode(_)), "{err:?}");
    assert!(err.to_string().contains("decode failed"));
}

#[test]
fn signal_kinds_and_caps_survive_the_wire() {
    let view = kitchen_sink();
    let back = View::from_json(&view.to_json().unwrap()).unwrap();
    let threats = back.signal("threats").expect("threats");
    assert_eq!(threats.kind, SignalKind::List);
    assert_eq!(threats.cap, Some(200));
}

#[test]
fn event_ops_survive_the_wire_including_nesting() {
    let view = kitchen_sink();
    let back = View::from_json(&view.to_json().unwrap()).unwrap();
    let Node::Element(el) = &back.root else { panic!() };
    assert_eq!(el.events.len(), 2);
    let click = &el.events[0];
    assert_eq!(click.event, "click");
    assert_eq!(click.action.ops.len(), 4);
    match &click.action.ops[3] {
        EventOp::Debounce { ms, ops } => {
            assert_eq!(*ms, 250);
            assert_eq!(ops.len(), 1);
        }
        other => panic!("debounce did not survive: {other:?}"),
    }
    assert!(click.action.ops[1].write_targets(&mut Vec::new()) == ());
}

#[test]
fn a_remote_handler_arrives_with_its_route_and_destination() {
    let view = kitchen_sink();
    let back = View::from_json(&view.to_json().unwrap()).unwrap();
    let h = back.handler("disable").expect("handler");
    assert!(h.is_remote());
    assert_eq!(h.writes.as_deref(), Some("rule"));
    let route = h.server.as_ref().unwrap();
    assert_eq!(route.method, "POST");
    assert_eq!(route.path, "/api/v1/rules/42/disable");
}

#[test]
fn a_native_expression_is_reported_before_it_can_break_the_wire() {
    let view = View::new(
        "client-only",
        Node::element("p").child(Node::text_expr(Expr::native(|_| Value::Str("hi".into())))),
    );
    assert!(!view.is_portable());
    let errs = view.portability_errors();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("Native"), "{errs:?}");
}

#[test]
fn serializing_a_native_expression_fails_loudly_instead_of_losing_it() {
    let expr = Expr::native(|_| Value::Null);
    let err = serde_json::to_string(&expr).unwrap_err();
    assert!(
        err.to_string().contains("client-only"),
        "expected a clear diagnostic, got: {err}"
    );
}

#[test]
fn raw_html_is_reported_as_non_portable() {
    let view = View::new("server-only", Node::element("div").child(Node::raw("<hr/>")));
    assert!(!view.is_portable());
    let errs = view.portability_errors();
    assert!(errs[0].contains("Node::Raw"), "{errs:?}");
}

#[test]
fn native_node_is_reported_as_non_portable_with_a_way_forward() {
    // The failure mode this guards: a `vhtml!` tree reaching the wire boundary.
    // It must fail loudly and say what to use instead, not serialize into a
    // view that renders nothing on the other side.
    let view = View::new(
        "maud",
        Node::element("ul").child(Node::native_render(|_| Node::missing())),
    );
    assert!(!view.is_portable());
    let errs = view.portability_errors();
    assert!(errs[0].contains("Node::Native"), "{errs:?}");
    assert!(errs[0].contains("Node::each"), "{errs:?}");
}

#[test]
fn serializing_a_native_node_fails_with_a_reason() {
    let node = Node::native_render(|_| Node::text("x"));
    let err = serde_json::to_string(&node).expect_err("a closure cannot cross the wire");
    let msg = err.to_string();
    assert!(msg.contains("client-only"), "{msg}");
}

#[test]
fn native_render_calls_the_closure_and_reads_the_resolver() {
    // Proves the closure is wired to a resolver rather than being opaque: this
    // is what makes a signal read inside it subscribe the surrounding effect.
    #[derive(Default)]
    struct OneSignal;
    impl crate::expr::Resolver for OneSignal {
        fn signal(&self, id: &str) -> Option<Value> {
            (id == "count").then_some(Value::Num(3.0))
        }
        fn payload(&self, _p: &ObjPath) -> Option<Value> {
            None
        }
        fn route_param(&self, _n: &str) -> Option<String> {
            None
        }
        fn query(&self, _n: &str) -> Option<String> {
            None
        }
        fn route(&self) -> String {
            String::new()
        }
    }

    let node = Node::native_render(|r| {
        let n = r.signal("count").and_then(|v| v.as_f64()).unwrap_or(0.0);
        Node::text(format!("{n} rows"))
    });

    let built = match &node {
        Node::Native(f) => f.call(&OneSignal),
        other => panic!("expected Node::Native, got {other:?}"),
    };
    assert_eq!(built, Node::text("3 rows"));

    // Two native nodes built from the same closure are equal; two from
    // different closures are not, which is what keeps reconciliation honest.
    assert_eq!(node, node.clone());
    let other = Node::native_render(|_| Node::text("x"));
    assert_ne!(node, other);
}

#[test]
fn value_survives_json_interop_including_non_finite_numbers() {
    let src: serde_json::Value = serde_json::json!({
        "n": 1,
        "f": 1.5,
        "s": "x",
        "b": true,
        "arr": [1, 2],
        "obj": { "k": "v" }
    });
    let value = Value::from_json(&src);
    assert_eq!(value.to_json(), src);

    // Non-finite values cannot be represented in JSON; degrade to null rather
    // than panicking or emitting invalid output.
    assert_eq!(Value::Num(f64::NAN).to_json(), serde_json::Value::Null);
    assert_eq!(Value::Num(f64::INFINITY).to_json(), serde_json::Value::Null);
}

#[test]
fn the_wire_format_is_reasonably_compact() {
    // Not a size assertion — a regression guard. A backend shipping a 400-row
    // admin view should not be paying for an order of magnitude of overhead.
    let json = kitchen_sink().to_json().unwrap();
    let plain = kitchen_sink();
    let budget = 4096;
    assert!(
        json.len() < budget,
        "kitchen-sink view serialised to {} bytes (budget {budget})",
        json.len()
    );
    let _ = plain;
}

#[test]
fn seed_island_is_valid_json_the_client_can_read_back() {
    let view = kitchen_sink();
    let html = crate::html::render_seed_island(
        &view,
        &crate::html::HtmlOptions::new()
            .with_signal("count", 7.0)
            .with_path("/threats")
            .with_param("id", "42"),
    );
    // Pull the body back out and confirm it decodes — this is exactly what the
    // browser does.
    let start = html.find('>').unwrap() + 1;
    let end = html.rfind("</script>").unwrap();
    let island = crate::parse_html::unescape(&html[start..end]);
    let parsed = crate::html::SeedIsland::from_json(&island).expect("island decodes");
    assert_eq!(parsed.path, "/threats");
    assert_eq!(parsed.params.get("id").map(String::as_str), Some("42"));
    assert!(matches!(
        parsed.signals.get("count"),
        Some(Value::Num(n)) if *n == 7.0
    ));
}
#[test]
fn composite_to_text_is_parseable_json() {
    // This is what a remote handler's request body is: the client evaluates the
    // body expression, calls `to_text`, and hands the result to `JSON.parse`.
    // If `to_text` on a composite is not valid JSON, the parse fails, the client
    // silently sends no body, and the server answers for a request it thinks
    // was empty. Nothing errors — the feature just quietly does nothing.
    let body = Value::map([
        ("q", Value::Str("checkout".into())),
        ("limit", Value::Num(10.0)),
        ("tags", Value::List(vec![Value::Str("a".into())])),
    ]);

    let text = body.to_text();
    let parsed: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{text:?} is not JSON: {e}"));

    assert_eq!(parsed["q"], "checkout");
    assert_eq!(parsed["limit"], 10);
    assert_eq!(parsed["tags"][0], "a");
}

#[test]
fn composite_to_text_escapes_what_json_requires() {
    // A filter containing a quote, a backslash, or a newline must not be able to
    // break out of the JSON it is embedded in.
    let nasty = "he said \"hi\"\\ then\nleft";
    let text = Value::map([("q", Value::Str(nasty.into()))]).to_text();
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{text:?} is not JSON: {e}"));
    assert_eq!(parsed["q"].as_str(), Some(nasty));
}

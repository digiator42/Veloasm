# velo_ast

**Portable, serializable reactive view AST.** The shared contract between backend and frontend.

## Architecture

```
BACKEND (Rust)                          FRONTEND (WASM)
─────────────                          ───────────────
┌─────────────────┐    JSON over wire    ┌─────────────────┐
│ velo_ast        │ ◄──────────────────► │ veloasm         │
│ (portable AST)  │   View.to_json()     │ (native runtime)│
│                 │                      │                 │
│ View            │                      │ mount()         │
│ SignalDecl      │                      │ signal!()       │
│ EventAction     │                      │ render_expr()   │
└─────────────────┘                      └─────────────────┘
```

## Why velo_ast?

| Scenario | Benefit |
|----------|---------|
| **SSR / Backend rendering** | Server builds `View` → JSON → client mounts live |
| **Backend-owned UI logic** | Security, validation, data shaping on server |
| **Dynamic UIs / CMS** | Store view definitions in DB, render on any client |
| **Micro-frontends** | Services emit compatible AST fragments |
| **Testing** | Assert on AST structure without browser |
| **Cross-platform** | Same AST → WASM, Native (Tauri), Server HTML |

## Core Types

```rust
use velo_ast::{View, Node, Expr, SignalDecl, EventAction, EventOp, Value};

// Build a portable view
let view = View::new("counter",
    Node::element("div")
        .attr("class", "panel")
        .child(Node::text("Count: "))
        .child(Node::text_expr(Expr::signal("count")))
        .child(Node::element("button")
            .event("click", EventAction::one(EventOp::add("count", 1.0)))
            .child(Node::text("+")))
);

// Declare signals the view reads/writes
view.with_signal(SignalDecl::count("count", 0.0))
    .declare_used_signals();

// Serialize to JSON for transport
let json = view.to_json()?;
```

## Signal Declarations

```rust
SignalDecl::count("name", initial)       // numeric, supports add/sub
SignalDecl::scalar("name", initial)      // any JSON value
SignalDecl::list("name", Vec::new())     // reactive list (SignalVec)
```

## Event Actions (Portable)

```rust
EventAction::one(EventOp::add("count", 1.0))     // count += 1
EventAction::one(EventOp::toggle("flag"))        // flag = !flag
EventAction::one(EventOp::set("name", value))    // name = value
EventAction::one(EventOp::push("items", value))  // items.push(value)
EventAction::one(EventOp::call("handler"))       // invoke registered handler
EventAction::one(EventOp::Navigate { to })       // client navigation
```

## Client Mounting

```rust
// From JSON (received from backend)
let view: View = View::from_json(&json)?;
let node = veloasm::ast::mount(&view);
veloasm::mount(node);  // attaches to <body>

// Or mount a bare Node tree (from vhtml! macro)
let node = vhtml! { div { "Hello" } };
veloasm::mount(veloasm::ast::mount_node(node));
```

## Features

- `serde` — Enable `Serialize`/`Deserialize` for `View`, `Node`, `Expr`, `EventAction` (default: off)
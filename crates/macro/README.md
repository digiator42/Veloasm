# vhtml!

**Maud-like syntax → portable `velo_ast::Node`.** Build reactive views that serialize to JSON and run on any client.

## When to Use

| Macro | Output | Use Case |
|-------|--------|----------|
| `vhtml!` | `velo_ast::Node` | Backend APIs, SSR, portable views, micro-frontends |
| `view!` | `veloasm::DomNode` | Client-only native WASM components |

## Quick Start

```rust
use veloasm::vhtml;
use veloasm::velo_ast::{View, SignalDecl, Expr};

// Build a portable view tree
let root = vhtml! {
    schema { count, name },           // compile-time signal contract
    div class="panel" {
        h1 { ${ Expr::signal("name") } }
        p { "Count: " ${ Expr::signal("count") } }
        button on:click={ add("count", 1) } { "+" }
        button on:click={ toggle("flag") } { "toggle" }
    }
};

// Wrap in View with signal declarations
let view = View::new("counter", root)
    .with_signal(SignalDecl::count("count", 0.0))
    .with_signal(SignalDecl::scalar("name", "Alice"))
    .declare_used_signals();

// Serialize and send to client
let json = view.to_json()?;
```

## Syntax Reference

### Elements & Attributes
```rust
div class="card" id="main" { "content" }
input type="text" disabled;
div .static-class { "box" }                    // class shorthand
div class:active=(cond) { "toggles" }          // reactive class
div class:active=|flag| { "pipe signal ref" }  // pipe sugar
```

### Reactive Text
```rust
p { "Count: " ${ Expr::signal("count") } }     // raw AST Expr (portable)
p { "Count: " (count.get()) }                  // native Rust (client-only)
```

### Event Handlers (Concise)
```rust
button on:click={ add("count", 1.0) } { "+" }
button on:click={ toggle("flag") } { "flip" }
button on:click={ set("name", str("Alice")) } { "set" }
button on:click={ || { add("count", 1); toggle("flag") } } { "multi" }
button on:click={ call("persist") } { "save" }
```

### Conditionals
```rust
@Show("flag") { p { "on" } } @else { p { "off" } }      // portable signal id
@Show(flag) { p { "on" } } @else { p { "off" } }        // bare identifier
Show when=|flag| fallback="off" { p { "on" } }          // component form
Suspense loading=|loading| fallback="..." { content }   // inverted
```

### Loops
```rust
@for i in 0..3 { li { (i) } }                          // unkeyed (native)
@for t in items, key = t.id { li { (t.title) } }       // keyed (portable)
```

### Schema (Compile-Time Contract)
```rust
vhtml! {
    schema { count, flag, user_name },  // every @Show/|pipe| must be declared
    @Show("count") { ... }              // OK
    @Show("missing") { ... }            // COMPILE ERROR
}
```

### Let Bindings
```rust
@let greeting = "Hello";
p { (greeting) }
```

## Pipeline

```
vhtml! { ... }           →  velo_ast::Node
       │
       ▼
View::new("name", node)  →  View (with SignalDecl)
       │
       ▼
view.to_json()           →  JSON string
       │
       ▼ (over HTTP/SSE)
Client receives JSON
       │
       ▼
veloasm::ast::mount(&view)  →  DomNode (live reactivity)
veloasm::mount(node)         →  attached to DOM
```

## Client-Side Mounting

```rust
// From backend JSON
let view: View = View::from_json(&json)?;
let node = veloasm::ast::mount(&view);
veloasm::mount(node);

// Or bare Node from vhtml!
let node = vhtml! { div { "Hello" } };
veloasm::mount(veloasm::ast::mount_node(node));
```

## Features

- Works on stable Rust
- Requires `veloasm` as dependency (re-exports `velo_ast`)
- Compile-time schema validation
- Zero WASM dependencies in the macro itself
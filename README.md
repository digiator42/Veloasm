# veloasm

[![crates.io](https://img.shields.io/crates/v/veloasm.svg)](https://crates.io/crates/veloasm)

Fine-grained reactive SPA framework for Rust → WebAssembly. No virtual DOM — state changes update the real DOM surgically.

> 📚 **Full documentation: [digiator42.github.io/Veloasm](https://digiator42.github.io/Veloasm/)**

## Install

```toml
[dependencies]
veloasm = "0.1"
```

```sh
cargo install veloasm-cli
veloasm new my-app
veloasm dev   # open http://localhost:8080
```

## Taste

```rust
use veloasm::prelude::*;

let count = signal!(0);   // Copy handle: get/set/update, zero clones

view! {
    <div>
        <h2>{ count }</h2>
        <button on:click={ () => count.update(|c| *c += 1) }>"Increment"</button>
    </div>
}
```

## Layout

- `crates/velo` — runtime (`veloasm` package): reactivity, DOM, router, fetch, error overlay.
- `crates/macro` — `view!`, `#[component]`, `routes!`, `#[route]`, `app!`, `vhtml!`.
- `crates/ast` — the shared, serializable view AST (`velo_ast`): written once on a backend, rendered as HTML *or* mounted as a live WASM client.
- `crates/velo-cli` — `veloasm new` / `dev` / `build` (wraps Trunk).
- `examples/` — runnable apps, plus real-world submodules (`luxshield`, `cyberflow`). Clone with `git clone --recurse-submodules`.

## The shared AST

Write a view **once**, in Rust, on a backend. Render it **twice**: as HTML for
the first paint, and as a live fine-grained WASM client in the browser.

```rust
// On the backend — a view is data.
let view = View::new("threat-panel", Node::element("section")
    .child(Node::text_expr(Expr::signal("count")))
    .child(Node::element("button").on("click", EventAction::one(EventOp::Update {
        target: "count".into(), op: ArithOp::Add,
        by: EventValue::Expr(Expr::Num(1.0)),
    }))));

let html = velo_ast::render(&view);              // SSR
let json = view.to_json()?;                     // the wire

// In the browser — the same tree, mounted reactively.
let view: velo_ast::View = fetch_json("/api/panel").await?;
veloast::mount(veloast::ast::mount(&view));
```

A click rewrites one text node. No reload, no round trip.

Maud is supported two ways: `vhtml!` takes Maud's syntax and lowers it to the
AST, and `velo_ast::parse_html` reads real Maud output back into a tree — so an
existing backend can migrate one panel at a time. `vhtml!` loops update from
signal writes; with a `key` (`@for t in rows.get(), key = t.id`) rows reconcile
by identity, so a reorder moves DOM instead of rebuilding it. An island can
declare its signal contract with a `schema { … }` header, checked at compile
time — with no header, no checks apply.

Run the client end to end in a real browser:

```sh
make browser-test
```

See [docs/src/shared-ast/overview.md](docs/src/shared-ast/overview.md).

## Status

Compiles to `wasm32-unknown-unknown`. See the [docs](https://digiator42.github.io/Veloasm/) for reactivity, routing, Suspense, and deployment.

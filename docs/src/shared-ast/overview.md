# The Shared AST

> Write a view **once, in Rust, on a backend**. Render it **twice**: as HTML
> for the first paint, and as a live, fine-grained WASM client in the browser.

## The problem this solves

`view!` compiles straight to Rust that manipulates `DomNode`. That is ideal
when the client and the server are the same codebase, and it means the view
**cannot leave the compiler** — there is no artifact a Rust backend can hand to
a browser.

So a backend that wants a live UI has two bad options:

1. **Ship HTML and let the server do everything.** Every interaction is a round
   trip, and the browser holds no state.
2. **Write the UI twice** — once in the backend's template language, once in
   the client framework. The two drift.

The shared AST is the third option. A view becomes **data** — a `View` value —
that a backend can build, render to HTML, and ship as JSON. The browser mounts
the *same* tree with live reactivity. One source, two renderers, no drift.

```text
                     ┌──────────────────────────────────┐
                     │      velo_ast::View   (data)      │
                     │  Node · Expr · Attr · EventAction │
                     └────┬──────────────────────┬──────┘
              render()    │                      │   render() + mount()
          (native / SSR) │                      │   (wasm / reactive)
             ┌───────────▼─────────┐   ┌────────▼─────────────────┐
             │ "<li class=…>Ada</li>"│   │  DomNode + RwSignal<Value>│
             │  first paint, no JS   │   │  one text node per change │
             └───────────────────────┘   └───────────────────────────┘
```

## The three parts

| Crate | What it is |
|---|---|
| `velo_ast` (`crates/ast`) | The data model, the SSR renderer, and an HTML parser. **Zero dependencies** by default; pure data, so it compiles for a native server *and* for `wasm32-unknown-unknown`. |
| `veloast::ast` (`crates/velo/src/ast/`) | The browser half: mounts a `View` with the framework's existing fine-grained primitives. |
| `veloast::vhtml!` (`crates/macro/src/vhtml.rs`) | Maud's template syntax, lowered to the AST. |

## Building a view

A view is a `View`: a tree, the signals it uses, the handlers it calls, and a
static payload.

```rust
use velo_ast::{Attr, Expr, EventAction, EventOp, Node, SignalDecl, View};

let view = View::new(
    "threat-panel",
    Node::element("section")
        .static_attr("class", "panel")
        .attr(Attr::class_toggle("is-on", Expr::signal("flag")))
        .child(Node::text_expr(Expr::signal("count")))
        .child(Node::each(
            Expr::signal("rows"),
            Expr::item_field(0, "id"),
            Node::element("li").child(Node::text_expr(Expr::item_field(0, "name"))),
        ))
        .child(
            Node::element("button").on(
                "click",
                EventAction::one(EventOp::Update {
                    target: "count".into(),
                    op: velo_ast::ArithOp::Add,
                    by: velo_ast::EventValue::Expr(Expr::Num(1.0)),
                }),
            ),
        ),
)
.with_signal(SignalDecl::count("count", 0.0))
.with_signal(SignalDecl::list("rows", rows));
```

Two things are worth noticing.

**A signal is addressed by name.** `Expr::signal("count")` is a *string*, not a
Rust handle — because a string is what survives a round trip through JSON. The
client resolves the name to a real reactive cell (see below).

**A handler is a list of operations, not a closure.** `EventOp::Update` is
`count += 1`; it runs in the browser, in WASM, with no network round trip. A
handler that *should* be a round trip names a server route instead.

## Signals: the part that needed solving

The framework's signals are typed: `RwSignal<T>` is a `Copy` pointer into a
leaked `SignalEngine<T>`. Nothing can look one up by name, so a name-addressed
AST had nowhere to point.

`veloast::ast::SignalStore` is the bridge. It instantiates
`RwSignal<Value>` — a real typed handle over the **existing** engine, with no
change to that engine — once per declared signal, and resolves ids to those
handles.

Everything downstream is then ordinary veloasm reactivity. `Expr::eval` calls
`RwSignal::get()`, which subscribes the running effect. **That single call is
what makes the whole renderer fine-grained**: an expression that reaches a
signal becomes a dependency automatically, with no explicit subscribe step
anywhere in the renderer.

`Value` is JSON-shaped (`Null`, `Bool`, `Num`, `Str`, `List`, `Map`), so one
signal can hold a whole table of server records as easily as a counter.

## Handlers: the feature lives on the server

A handler is either local or remote.

```rust
// Local: runs in the browser, in WASM. A click that updates a counter.
EventOp::Update { target: "count".into(), op: ArithOp::Add, by: /* … */ }

// Remote: the backend owns the logic. The client sends a request and writes
// the reply into a signal.
.with_handler(HandlerRef::remote(
    "acknowledge",
    ServerRoute::post("/api/v1/threats/42/ack"),
    "threats",
))
```

The remote form is the point of the whole exercise. A feature authored once in
a Rust backend behaves as a live, reactive UI in the browser — and none of its
logic is ever compiled to WASM.

## Rendering it

### On the server

```rust
let html = velo_ast::render(&view);
```

`render` escapes every text run and every attribute value. The only unescaped
content is `Node::Raw`, which exists precisely so a backend can inject markup
it produced itself.

### In the browser

```rust
let view: velo_ast::View = veloasm::fetch_json("/api/panel").await?;
veloast::mount(veloast::ast::mount(&view));
```

`veloast::ast::mount` returns a `DomNode`; effects and listeners are scoped to
it, so unmounting disposes everything the view created — the same guarantee
`#[component]` gives.

## Maud, without pretending it can be intercepted

Maud's `html!` is a `macro_rules!` macro. Once it has run, all that is left is
a `Markup` string builder — there is no way to intercept its token stream and
turn it into something reactive. Any claim to the contrary is wrong.

So there are two honest paths, and the framework supports both.

**`vhtml!` — Maud's syntax, compiled to the AST.** A *sibling* macro, not a
wrapper. It accepts the same surface syntax and lowers to `Node`:

```rust
use veloasm::{signal, vhtml};

struct Threat {
    id: u32,
    title: String,
    severity: &'static str,
}

let threats = signal!(vec![Threat::new(1, "brute force", "high")]);

let root = vhtml! {
    section class="panel" {
        h2 { "Live threats" }

        @if total > 0 {
            span class="badge" { (total) " open" }
        } @else {
            span class="badge danger" { "none" }
        }

        @for t in threats.get() {
            li class:critical=(t.severity == "high") { (t.title) }
        }

        input type="text" placeholder="Filter" disabled;
    }
};

veloasm::mount(veloasm::ast::mount_node(root));
```

Note what is *not* in that example: no `Value`, no `ObjPath`, no field-path
string. The loop variable is an ordinary Rust value, its fields are read
directly, and the class toggle is a Rust comparison — because the loop body is
compiled into a closure rather than into AST nodes. That is the whole point of
the Maud path: a Maud developer should not have to learn a data format to get
started.

Three deliberate departures from Maud:

- **`@Show`** is new. Maud has no reactive concept, so nothing names it.
  `@Show("sig") { … } @else { … }` swaps branches when the signal flips.
- **`on:event=(action)`** is new. An attribute named `on:<event>` attaches an
  `EventAction` instead of writing an attribute. Plain `onclick="…"` still
  writes an attribute, so `hx-*` attributes keep working untouched.
- **`class:name=(cond)`** is supported, so a conditional class reads the way it
  does elsewhere. Any other `x:y=` form is a compile error naming the two forms
  it accepts, rather than being silently reinterpreted.

**`parse_html` — real Maud output, read back.** If you already have Maud
markup, it parses straight into the AST:

```rust
let root = velo_ast::parse_html::parse(&markup.render().0)?;
```

Structure survives exactly; behaviour does not, because HTML does not carry it.
That makes a **panel-by-panel migration** possible: ship the structure now,
add handlers as you go.

## What is portable, and what is not

Two escape hatches, both explicit:

- `Expr::Native` — arbitrary Rust, evaluated in the browser. **Never
  serialized.** `View::is_portable()` reports it, and `View::portability_errors()`
  says exactly where.
- `Node::Raw` — pre-rendered HTML. **Never serializable.** Server-only by
  construction.

A `vhtml!` tree uses `Expr::Native` for every splice, so it is client-only. A
view built with the builders is portable, and therefore shippable. The AST
refuses to blur that line: serializing a `Native` expression is an **error**,
not a silent placeholder.

## See it run

```sh
cd examples/ast-dashboard
cargo run --example emit_view -- --emit-view   # write public/view.json
trunk serve                                    # then open the page
```

The page fetches `view.json`, mounts it, and **runs its own assertions against
the live DOM** — dispatching real events and checking the resulting markup. The
verdicts appear at the bottom of the page.

To run the same check from a terminal:

```sh
make browser-test
```

It builds the example, serves it, drives it with headless Chrome, and fails if
any assertion failed.

## Caveats worth knowing

- **`RwSignal` engines are never freed.** Mounting the same view repeatedly
  leaks one small engine per signal per mount. This is the framework's
  existing trade (see `RwSignal`), inherited rather than introduced — mount
  views once per navigation, which is the intended usage.
- **The SSR output is the client's *initial state*, not the client's DOM.** The
  client mounts the AST on top of it; it does not diff against it. That keeps
  the renderer small and total, at the cost of not saving the first-frame round
  trip. Hydration markers can be added later without changing the AST.
- **`vhtml!` unkeyed loops are reactive but not reconciling.** Writing the
  list's signal re-runs the whole `@for` body, so rows appear and disappear
  correctly — this is a `Node::Native` closure, and it is why the loop can keep
  Maud's plain-Rust body. What it cannot do is *reuse* a row's DOM node: reuse
  requires comparing row values, and an unkeyed `vhtml!` row is an arbitrary
  Rust type rather than a comparable `Value`. Rows are therefore rebuilt on
  every list update.
- **`vhtml!` keyed loops reconcile.** `@for t in rows.get(), key = t.id` lowers
  to the same `Node::each` the builder API produces, so moved rows are placed
  rather than rebuilt. The price: rows are values (`Vec<Value>`-shaped
  sources), splices in the body constrain to the row grammar (reads, `t.field`
  / `t.get("f")`, literals, comparisons, arithmetic, boolean logic, ternaries,
  and `format!` with plain placeholders), and an inner keyed loop iterates a
  field read of the outer row (`@for r in t.children, key = r.code`).
  Anything else fails the build with the shape spelled out. An island can
  declare its signal contract with a `schema { … }` header, checked at
  compile time against `@Show("id")` literal reads — absent the header, no
  checks apply and nothing about existing behavior changes.

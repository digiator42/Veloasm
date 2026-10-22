//! `vhtml!` — Maud's syntax, compiled to the shared AST.
//!
//! Maud's `html!` is a `macro_rules!` macro. Once it has run, all that is left is
//! a `Markup` string builder, so there is no way to intercept its token stream
//! and turn it into something reactive. `vhtml!` is therefore a **sibling**
//! macro: it accepts the same surface syntax and lowers to
//! [`velo_ast`](velo_ast)'s [`Node`](velo_ast::Node), which the framework mounts
//! with live fine-grained effects — and which the same crate can also render to
//! an HTML string.
//!
//! ```ignore
//! let root = vhtml! {
//!     schema { threats_present = false },
//!     div class="panel" {
//!         h2 { "Live threats" }
//!         @Show("threats") {
//!             ul {
//!                 @for t in threats, key = t.id {
//!                     li .critical { t.title }
//!                 }
//!             }
//!         } @else {
//!             p { "No threats." }
//!         }
//!     }
//! };
//! veloast::ast::mount(&View::new("panel", root));
//! ```
//!
//! ## What maps to what
//!
//! | Maud | AST | notes |
//! |---|---|---|
//! | `div class="a" { … }` | `Node::Element` | |
//! | `"text"` | `Node::Text` | static, no effect |
//! | `(expr)` or bare `expr` | `Expr::render` | arbitrary Rust; client-only |
//! | `@if` / `@else if` / `@else` | `Node::If` | Rust decides at build time |
//! | `@for x in xs { … }` | a `Node::fragment` of rows | see *Loops* below |
//! | `@for x in xs, key = x.id { … }` | `Node::each` | keyed; moved/patched rows at mount |
//! | `schema { n = 3, hot } { … }` | a compile-time contract | see *The schema* below |
//! | `@let y = …;` | a Rust `let` | lexical only |
//! | `.cls` | `Attr::class_toggle` | Maud's class shorthand |
//! | `input type="text"` | void element | no block needed |
//! | `// comment` | dropped | Maud line comments |
//!
//! ## The schema
//!
//! An optional `schema { … }` header declares the signals an island may name.
//! String-literal signal ids — the `@Show("count")` form — are checked against
//! it **while the macro runs**: an id that is not in the schema fails the build
//! with the exact spelling of the offending reference, instead of mounting a
//! view that silently renders that branch as null. The runtime counterpart is
//! still `View::new(name, root).with_signal(…)`; the schema is the macro's
//! side of the same contract, stated where the typo actually lives.
//!
//! ```ignore
//! vhtml! {
//!     schema { count = 3, hot },
//!     p class:hot=("count > 1") { (count) }
//! }
//! ```
//!
//! The schema is enforcement-through-presence: no header, and `@Show` ids are
//! checked by nothing but runtime ("an undeclared read renders empty"); a
//! header turns every literal id in the body into a build-time requirement.
//!
//! ## The deliberate departures from Maud
//!
//! **`@Show` is new.** Maud has no reactive concept, so nothing names this.
//! `@Show("sig") { … } @else { … }` swaps branches when the signal flips; that
//! is the reactive counterpart of `@if`. The condition is either a signal id
//! string — *portable*, and therefore shippable — or a Rust condition.
//!
//! **`on:event=(action)` is new.** An attribute named `on:<event>` attaches an
//! [`EventAction`](velo_ast::EventAction) instead of writing an attribute.
//! Plain `onclick="…"` still writes an attribute exactly as in Maud, so `hx-*`
//! attributes keep working untouched.
//!
//! ## Loops
//!
//! `@for` is **reactive**. The loop body is rebuilt inside an effect, so a
//! signal read while collecting rows re-runs the whole loop:
//!
//! ```ignore
//! @for t in threats.get() {
//!     li class:critical={ t.severity == "high" } { (t.title) }
//! }
//! ```
//!
//! That is the important part: the loop keeps Maud's *plain Rust* body — the
//! loop variable, field access, comparisons, `format!` — instead of forcing
//! every item through `Value`. The cost is a `&dyn Resolver` closure, so the
//! body may only use owned values or signals; it cannot borrow a local that
//! dies with the surrounding function.
//!
//! Rows are rebuilt rather than reconciled — every update re-runs the body and
//! replaces the fragment wholesale. That tradeoff is deliberate: reuse requires
//! comparing row *values*, and a row here is an arbitrary Rust type, not a
//! comparable `Value`. On a list that churns, add a key clause:
//!
//! ```ignore
//! @for r in rows, key = r.id {
//!     li { r.label }
//! }
//! ```
//!
//! A keyed `@for` lowers to [`Node::each`](velo_ast::Node::each), the
//! framework's keyed reconciling loop: rows are matched by key, removed rows
//! let go of their DOM nodes and their row-scoped effects, and moved rows are
//! *placed* rather than rebuilt — untouched scroll positions, no re-created
//! inputs, no lost focus. The key is a value path over the row (`t.id`,
//! `t.meta.version`, the literal `index`), evaluated as data by the runtime.
//!
//! The cost of a key clause is that the body must speak *value*: anything that
//! touches the row must be a read (`t.name`, `t.get("name")`), a comparison,
//! arithmetic, boolean logic, or a `format!` whose placeholders are plain
//! `{}`/`{name}` — because those are the shapes that lower to serializable
//! [`Expr`](velo_ast::Expr) trees, and the reconciler re-renders rows from a
//! *template*, not from Rust closures capturing one row per build. A keyed body
//! that mixes an unsupported Rust shape over the row is rejected at compile
//! time with the offending expression quoted; splices that do not mention the
//! row at all stay ordinary Rust.
//!
//! ## Portability
//!
//! Every `(expr)` splice becomes an [`Expr::Native`](velo_ast::Expr::Native),
//! and every unkeyed `@for` becomes a [`Node::Native`](velo_ast::Node::Native)
//! Rust closure. Neither **can be serialized**, so a `vhtml!` tree is
//! client-only. To build a view a *backend* can ship, use the `velo_ast`
//! builders directly — that is the other half of this feature, and the half
//! that crosses the wire. (A keyed `@for` whose body touches the row only
//! through value reads *can* serialize, but a `vhtml!` island is not expected
//! to try — ship it via a matching `velo_ast` tree when that matters.)

use proc_macro::TokenStream;
use proc_macro2::{TokenStream as TokenStream2, Spacing, TokenTree};
use quote::quote;
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream, Result};
use syn::parse::discouraged::Speculative;
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, Expr as RustExpr, Ident, LitStr, Pat, Token};
use syn::token::Paren;

/// Two-argument result alias for the lowering code, which reports via row
/// grammar errors; `Result` stays syn's one-argument alias for the parser.
type Res<T> = std::result::Result<T, syn::Error>;

/// Event helper functions available in the vhtml! macro scope.
/// These generate EventAction/EventOp tokens for concise event handlers.
fn event_helpers() -> TokenStream2 {
    quote! {
        /// Add to a signal: `count += n`
        fn add(target: &str, by: f64) -> ::veloasm::velo_ast::event::EventAction {
            ::veloasm::velo_ast::event::EventAction::one(::veloasm::velo_ast::event::EventOp::add(target, by))
        }
        /// Subtract from a signal: `count -= n`
        fn sub(target: &str, by: f64) -> ::veloasm::velo_ast::event::EventAction {
            ::veloasm::velo_ast::event::EventAction::one(::veloasm::velo_ast::event::EventOp::sub(target, by))
        }
        /// Toggle a boolean signal
        fn toggle(target: &str) -> ::veloasm::velo_ast::event::EventAction {
            ::veloasm::velo_ast::event::EventAction::one(::veloasm::velo_ast::event::EventOp::toggle(target))
        }
        /// Set a signal to an expression value
        fn set(target: &str, value: ::veloasm::velo_ast::Expr) -> ::veloasm::velo_ast::event::EventAction {
            ::veloasm::velo_ast::event::EventAction::one(::veloasm::velo_ast::event::EventOp::set(target, ::veloasm::velo_ast::event::EventValue::Expr(value)))
        }
        /// Call a handler by name
        fn call(handler: &str) -> ::veloasm::velo_ast::event::EventAction {
            ::veloasm::velo_ast::event::EventAction::one(::veloasm::velo_ast::event::EventOp::call(handler))
        }
        /// Navigate to a route
        fn navigate(to: impl Into<::veloasm::velo_ast::Expr>) -> ::veloasm::velo_ast::event::EventAction {
            ::veloasm::velo_ast::event::EventAction::one(::veloasm::velo_ast::event::EventOp::Navigate { to: to.into() })
        }
        /// Allow default browser behavior (don't preventDefault)
        fn allow_default() -> ::veloasm::velo_ast::event::EventAction {
            ::veloasm::velo_ast::event::EventAction::one(::veloasm::velo_ast::event::EventOp::AllowDefault)
        }
        /// Combine multiple EventActions into one
        fn combine(actions: Vec<::veloasm::velo_ast::event::EventAction>) -> ::veloasm::velo_ast::event::EventAction {
            let mut ops = Vec::new();
            for a in actions { ops.extend(a.ops); }
            ::veloasm::velo_ast::event::EventAction::new(ops)
        }
        /// Create an EventValue from an expression
        fn ev_expr(expr: ::veloasm::velo_ast::Expr) -> ::veloasm::velo_ast::event::EventValue {
            ::veloasm::velo_ast::event::EventValue::Expr(expr)
        }
        /// Create an EventValue from a target property (e.g. "value", "checked")
        fn ev_target_prop(prop: &str) -> ::veloasm::velo_ast::event::EventValue {
            ::veloasm::velo_ast::event::EventValue::TargetProperty(prop.to_string())
        }
        /// Create an EventValue from the event target's text
        fn ev_target_text() -> ::veloasm::velo_ast::event::EventValue {
            ::veloasm::velo_ast::event::EventValue::TargetText
        }
        /// Create an EventValue from the last server response
        fn ev_response() -> ::veloasm::velo_ast::event::EventValue {
            ::veloasm::velo_ast::event::EventValue::Response
        }
        /// Signal reference for use in EventValue::Expr
        fn signal(id: &str) -> ::veloasm::velo_ast::Expr {
            ::veloasm::velo_ast::Expr::signal(id)
        }
        /// Numeric literal as Expr
        fn num(n: f64) -> ::veloasm::velo_ast::Expr {
            ::veloasm::velo_ast::Expr::Num(n)
        }
        /// String literal as Expr
        fn str(s: &str) -> ::veloasm::velo_ast::Expr {
            ::veloasm::velo_ast::Expr::str(s)
        }
        /// Boolean literal as Expr
        fn bool(b: bool) -> ::veloasm::velo_ast::Expr {
            ::veloasm::velo_ast::Expr::Bool(b)
        }
    }
}

/// A comma-separated argument list, for re-parsing macro tokens.
struct MacroArgs(pub Punctuated<RustExpr, Token![,]>);

impl Parse for MacroArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(MacroArgs(Punctuated::parse_terminated(input)?))
    }
}

/// One attribute on an element.
struct MAttr {
    /// Empty when this is an event binding — the event name lives in `event`.
    name: String,
    /// `None` is a bare boolean attribute (`input disabled`).
    value: Option<AttrValueSrc>,
    /// `on:<event>` — the Velo event extension.
    event: Option<String>,
}

/// The right-hand side of an attribute.
enum AttrValueSrc {
    /// `"literal"`
    Lit(LitStr),
    /// `(expr)` — rendered into a reactive expression.
    Splice(RustExpr),
    /// `{{ expr }}` — a raw `velo_ast::Expr` used directly.
    RawExpr(RustExpr),
    /// `.cls` — Maud's class shorthand, a statically-on toggle.
    ClassLit(String),
    /// `class:cls=(expr)` — a conditional class toggle.
    ClassCond { cls: String, expr: RustExpr, is_raw: bool },
    /// `${ expr }` — a raw `velo_ast::Expr` used directly (JS template-literal style).
    DollarExpr(RustExpr),
}

/// A parsed node.
enum MNode {
    /// `"text"`
    Text(LitStr),
    /// `(expr)`, or a bare expression, in text position — lowered through
    /// `emit_render` (wrapped in `Expr::render`).
    Splice(RustExpr),
    /// `{{ expr }}` — a raw `velo_ast::Expr` used directly, not wrapped.
    /// Needed to read AST signals by id in text position (`{{ Expr::signal("count") }}`).
    RawExpr(RustExpr),
    /// `${ expr }` — a raw `velo_ast::Expr` used directly (JS template-literal style).
    /// An alternative to `{{ }}` that reads better for developers coming from JS.
    DollarExpr(RustExpr),
    /// `@let name = expr;`
    Let {
        /// The bound name.
        name: Ident,
        /// The bound value.
        value: Box<RustExpr>,
    },
    Element(MElement),
    /// `@if` / `@else if`
    If {
        /// The condition, in Rust.
        test: RustExpr,
        /// The taken branch.
        then: Vec<MNode>,
        /// The `else` branch; a nested `If` for an `else if` chain.
        otherwise: Vec<MNode>,
        /// `false` when no `@else` was written, so the missing branch renders
        /// nothing rather than an empty text node.
        has_else: bool,
    },
    /// `@Show` — the reactive conditional.
    Show {
        /// Either a signal id string (portable) or a Rust condition.
        when: ShowCond,
        /// Shown while truthy.
        then: Vec<MNode>,
        /// Shown while falsey.
        otherwise: Vec<MNode>,
        /// `false` when no `@else` was written.
        has_else: bool,
    },
    /// `@for`
    For {
        /// The binding pattern.
        pat: Box<Pat>,
        /// The iterable.
        iter: RustExpr,
        /// `key = t.path`, spelled at the call site. A `Some` turns the loop
        /// into [`Node::Each`](velo_ast::Node::Each), the keyed reconciler.
        key: Option<KeyPath>,
        /// The body, evaluated once per item.
        body: Vec<MNode>,
    },
}

/// The condition of an `@Show`.
enum ShowCond {
    /// `@Show("count")` — a portable signal read. The literal is kept so the
    /// schema checker can point at the exact reference it rejects.
    Signal(LitStr),
    /// `@Show(cond)` or `@Show(move || cond)` — a Rust test.
    Rust(RustExpr),
}

/// `key = t.id` — the row-identifying clause of a keyed `@for`.
///
/// A key is a *value path*: the binding of this (or an enclosing keyed) loop,
/// then zero or more field steps, so `key = t` is valid — the row itself is the
/// identity — and `key = index` switches to positional identity explicitly.
/// Path literals, not expressions: the key has to be evaluable as an
/// [`Expr`](velo_ast::Expr) for the reconciler to use it, and that limit is
/// exactly what keeps the difference cheap.
struct KeyPath {
    /// The binding the path starts at — `t`, or `index` for positional.
    base: String,
    /// Field steps after the base: `t.meta.version` — `["meta", "version"]`.
    steps: Vec<String>,
    /// Where the `base` was written, for reporting a missing binding.
    base_span: proc_macro2::Span,
}

/// A parsed element.
struct MElement {
    /// The tag name. A leading capital makes this a component reference, so the
    /// runtime resolves it against its component table instead of the DOM.
    tag: String,
    /// Resolve through the component table rather than `document.createElement`.
    is_component: bool,
    /// Attributes, in source order.
    attrs: Vec<MAttr>,
    /// Child nodes.
    children: Vec<MNode>,
}

/// The `schema { … }` header — the compile-time signal contract.
struct Schema {
    /// Signal ids the island may name, in declaration order.
    names: Vec<String>,
    /// Whether the header was written at all. Enforcement is
    /// through-presence: no header, and literal `@Show` ids are unchecked.
    present: bool,
}

/// One loop binding visible to row expressions.
///
/// Unkeyed frames exist only so a Rust variable with the same name as a keyed
/// binding shadows it again — a row expression naming it must not be
/// rewritten to `r.item(…)`. Depth is not stored: it is the count of keyed
/// frames newer than the one being read, which is only known at the
/// reference site.
#[derive(Clone)]
struct Frame {
    name: String,
    /// `(item, i)` patterns bind the row index as `i`.
    index: Option<String>,
    keyed: bool,
}
// =============================================================================
// Parsing
// =============================================================================

/// Parse attributes up to `{`, `/`, or `;`.
struct AttrList(Punctuated<MAttr, Token![,]>);

impl Parse for AttrList {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut items = Punctuated::new();
        while !input.is_empty()
            && !input.peek(syn::token::Brace)
            && !input.peek(Token![;])
            && !input.peek(Token![/])
        {
            items.push(input.parse()?);
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(AttrList(items))
    }
}

impl Parse for MAttr {
    fn parse(input: ParseStream) -> Result<Self> {
        // `.cls` — Maud's class shorthand: a statically-on class toggle.
        if input.peek(Token![.]) && !input.peek2(Token![=]) {
            input.parse::<Token![.]>()?;
            let cls = input.call(Ident::parse_any)?;
            return Ok(MAttr {
                name: "class".into(),
                value: Some(AttrValueSrc::ClassLit(cls.to_string())),
                event: None,
            });
        }

        let mut first = input.call(Ident::parse_any)?.to_string();

        // `data-id`, `aria-label` — dash-cased HTML names are one name.
        while input.peek(Token![-]) && input.peek2(syn::Ident) {
            input.parse::<Token![-]>()?;
            let seg = input.call(Ident::parse_any)?.to_string();
            first.push('-');
            first.push_str(&seg);
        }

        // Two `name:value` forms, and only two. Anything else with a colon is
        // a mistake worth reporting rather than silently misreading — which is
        // what treating every `x:y` as an event used to do to `class:critical`.
        if input.peek(Token![:]) && !input.peek2(Token![=]) {
            let colon = input.parse::<Token![:]>()?.span;
            let second = input.call(Ident::parse_any)?.to_string();
            input.parse::<Token![=]>()?;
            let value = parse_attr_rhs(input)?;

            return match first.as_str() {
                // `on:click=(action)` — the Velo event extension.
                "on" => Ok(MAttr {
                    name: String::new(),
                    value: Some(value),
                    event: Some(second),
                }),
                // `class:critical=(cond)` — the class toggle, spelled the way
                // it is in every templating language.
                "class" => match value {
                    AttrValueSrc::Splice(expr) => Ok(MAttr {
                        name: String::new(),
                        value: Some(AttrValueSrc::ClassCond {
                            cls: second,
                            expr,
                            is_raw: false,
                        }),
                        event: None,
                    }),
                    AttrValueSrc::RawExpr(expr) => Ok(MAttr {
                        name: String::new(),
                        value: Some(AttrValueSrc::ClassCond {
                            cls: second,
                            expr,
                            is_raw: true,
                        }),
                        event: None,
                    }),
                    AttrValueSrc::DollarExpr(expr) => Ok(MAttr {
                        name: String::new(),
                        value: Some(AttrValueSrc::ClassCond {
                            cls: second,
                            expr,
                            is_raw: true,
                        }),
                        event: None,
                    }),
                    _ => Err(syn::Error::new(
                        colon,
                        "a class toggle needs a condition: `class:name=(expr)`",
                    )),
                },
                other => Err(syn::Error::new(
                    colon,
                    format!(
                        "unknown `{other}:` attribute form — write `on:{second}=…` for an \
                         event, or `class:{second}=…` for a class toggle"
                    ),
                )),
            };
        }

        if input.peek(Token![=]) {
            input.parse::<Token![=]>()?;
            return Ok(MAttr {
                name: first,
                value: Some(parse_attr_rhs(input)?),
                event: None,
            });
        }

        // Bare attribute: Maud's "present, no value".
        Ok(MAttr {
            name: first,
            value: None,
            event: None,
        })
    }
}

/// `= "literal"` or `= (expr)` or `= {{ expr }}` or `= ${ expr }` or `= |flag|` (raw Expr).
fn parse_attr_rhs(input: ParseStream) -> Result<AttrValueSrc> {
    // `${ expr }` — raw Expr (JS template-literal style).
    {
        let fork = input.fork();
        if let Ok(TokenTree::Punct(p)) = fork.parse::<TokenTree>() {
            if p.as_char() == '$'
                && p.spacing() == Spacing::Alone
                && fork.peek(syn::token::Brace)
            {
                input.advance_to(&fork);
                let content;
                syn::braced!(content in input);
                let expr: RustExpr = content.parse()?;
                return Ok(AttrValueSrc::DollarExpr(expr));
            }
        }
    }
    // `|flag|` — pipe-sugar for a signal id (cleaner than quoted string).
    if input.peek(Token![|]) {
        input.parse::<Token![|]>()?;
        let ident: Ident = input.parse()?;
        input.parse::<Token![|]>()?;
        let lit = LitStr::new(&ident.to_string(), ident.span());
        return Ok(AttrValueSrc::DollarExpr(
            syn::parse_quote!(::veloasm::velo_ast::Expr::signal(#lit))
        ));
    }
    if input.peek(syn::token::Brace) {
        let content;
        syn::braced!(content in input);
        // `{{ expr }}` — outer braces consumed, inner content starts with `{`.
        if content.peek(syn::token::Brace) {
            let inner;
            syn::braced!(inner in content);
            let expr: RustExpr = inner.parse()?;
            return Ok(AttrValueSrc::RawExpr(expr));
        }
        // A lone `{ ... }` in attribute position.
        return Ok(AttrValueSrc::Splice(content.parse()?));
    }
    if input.peek(LitStr) {
        return Ok(AttrValueSrc::Lit(input.parse()?));
    }
    let content;
    syn::parenthesized!(content in input);
    Ok(AttrValueSrc::Splice(content.parse()?))
}

/// Parse children until the matching `}`.
fn parse_children(input: ParseStream) -> Result<Vec<MNode>> {
    let mut out = Vec::new();
    while !input.is_empty() {
        out.push(parse_node(input)?);
    }
    Ok(out)
}

fn parse_node(input: ParseStream) -> Result<MNode> {
    // `//` line comment — Maud's comment form.
    if input.peek(Token![/]) && input.peek2(Token![/]) {
        input.parse::<Token![/]>()?;
        input.parse::<Token![/]>()?;
        while !input.is_empty() && !input.peek(Token![;]) && !input.peek(syn::token::Brace) {
            input.parse::<proc_macro2::TokenTree>()?;
        }
        return parse_node(input);
    }

    if input.peek(Token![@]) {
        input.parse::<Token![@]>()?;
        let kw = input.call(Ident::parse_any)?.to_string();
        return match kw.as_str() {
            "if" => parse_if(input),
            "for" => parse_for(input),
            "let" => parse_let(input),
            "Show" => {
                let when = parse_show_cond(input)?;
                let then = parse_block(input)?;
                let (otherwise, has_else) = parse_else(input, true)?;
                Ok(MNode::Show {
                    when,
                    then,
                    otherwise,
                    has_else,
                })
            }
            other => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!(
                    "vhtml: unknown `@{other}`; expected one of @if, @else, @for, @let, @Show"
                ),
            )),
        };
    }

    // `${ expr }` — a raw `velo_ast::Expr` used directly (JS template-literal style).
    {
        let fork = input.fork();
        if let Ok(TokenTree::Punct(p)) = fork.parse::<TokenTree>() {
            if p.as_char() == '$'
                && p.spacing() == Spacing::Alone
                && fork.peek(syn::token::Brace)
            {
                input.advance_to(&fork);
                let content;
                syn::braced!(content in input);
                let expr: RustExpr = content.parse()?;
                return Ok(MNode::DollarExpr(expr));
            }
        }
    }

    // `{{ expr }}` — a raw `velo_ast::Expr` used directly (not wrapped in
    // `Expr::render`). Needed to read AST signals by id in text position.
    if input.peek(syn::token::Brace) {
        let content;
        syn::braced!(content in input);
        // `{{ expr }}` — outer braces consumed, inner content starts with `{`.
        if content.peek(syn::token::Brace) {
            let inner;
            syn::braced!(inner in content);
            let expr: RustExpr = inner.parse()?;
            return Ok(MNode::RawExpr(expr));
        }
        // A lone `{ ... }` in text position isn't valid vhtml! syntax.
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "vhtml: unexpected `{ ... }` in text position; use `(expr)` for \
             reactive text or `{{ expr }}` for a raw velo_ast::Expr",
        ));
    }

    // `(expr)` splice in text position.
    if input.peek(syn::token::Paren) {
        let content;
        syn::parenthesized!(content in input);
        return Ok(MNode::Splice(content.parse()?));
    }

    // `"literal"` text.
    if input.peek(LitStr) {
        return Ok(MNode::Text(input.parse()?));
    }

// An element: `tag attr* { … }`, or `tag attr*;` for a childless element.
//
// The lookahead is what separates `section class="x" { … }` (an element) from
// `count` (a bare expression). An identifier is an element when it is followed
// by a block or by something that can start an attribute; otherwise it is a
// value. Maud resolves `.cls` the same way, so `foo.bar` is a class shorthand
// rather than a field access — which is why the rule is stated here rather than
// left implicit.
    // `type`, `ref`, `match` and friends are Rust keywords, so the lookahead has
    // to accept *any* identifier — `syn::Ident` alone would reject the very
    // attribute names HTML uses most.
    if input.peek(Ident::peek_any) && starts_element(input) {
        let tag: Ident = input.call(Ident::parse_any)?;
        let attrs: AttrList = input.parse()?;

        let children = if input.peek(syn::token::Brace) {
            parse_block(input)?
        } else {
            // Maud terminates a childless element with `;`. Accepting the
            // terminator (and then requiring nothing) is what makes
            // `input type="text";` and `br` both valid.
            if input.peek(Token![;]) {
                input.parse::<Token![;]>()?;
            }
            Vec::new()
        };

        // A leading capital means the tag names a registered component rather
        // than a DOM element, so the runtime resolves it against its component
        // table instead of `document.createElement`.
        let name = tag.to_string();
        let is_component = name.chars().next().is_some_and(|c| c.is_uppercase());
        return Ok(MNode::Element(MElement {
            // Components keep their exact spelling (the runtime matches names
            // case-sensitively); HTML tags are lowercased.
            tag: if is_component {
                name.clone()
            } else {
                name.to_ascii_lowercase()
            },
            is_component,
            attrs: attrs.0.into_iter().collect(),
            children,
        }));
    }

    // A bare expression — Maud allows `{ expr }`; accepting it unparenthesized
    // keeps `div { count }` readable.
    if input.peek(Ident::peek_any) {
        let fork = input.fork();
        if fork.parse::<RustExpr>().is_ok() {
            return Ok(MNode::Splice(input.parse()?));
        }
    }

    Err(input.error(
        "vhtml: expected an element, text, `(expr)`, or an @if / @for / @Show block",
    ))
}

/// Does the identifier at the cursor start an element rather than a value?
fn starts_element(input: ParseStream) -> bool {
    // `tag { … }` — an element with a block.
    if input.peek2(syn::token::Brace) {
        return true;
    }
    // `tag attr` / `tag attr="v"` / `tag .cls` — an element with attributes.
    // `Ident::peek_any` is required here: `type` is a Rust keyword, and
    // `type="text"` is one of the most common attributes in the language.
    if input.peek2(Ident::peek_any) || input.peek2(LitStr) || input.peek2(Token![.]) {
        return true;
    }
    // `tag="v"` — an element with a single attribute and no space.
    input.peek2(Token![=])
}

fn parse_let(input: ParseStream) -> Result<MNode> {
    let pat = Pat::parse_single(input)?;
    input.parse::<Token![=]>()?;
    let value: RustExpr = input.parse()?;
    if input.peek(Token![;]) {
        input.parse::<Token![;]>()?;
    }
    let name = match &pat {
        Pat::Ident(i) => i.ident.clone(),
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "vhtml: `@let` expects a plain identifier",
            ))
        }
    };
    Ok(MNode::Let { name, value: Box::new(value) })
}

/// `@Show(cond)` — reactive conditional. `cond` is either a signal id string
/// (`@Show("active")`, portable across the wire) or any Rust expression
/// (`@Show(count.get() > 0)`, reactive in-browser only). Parentheses are
/// optional for a bare signal id: `@Show active` is sugar for `@Show("active")`.
fn parse_show_cond(input: ParseStream) -> Result<ShowCond> {
    // `@Show("active")` — portable signal id.
    if input.peek(LitStr) {
        return Ok(ShowCond::Signal(input.parse::<LitStr>()?));
    }
    // `@Show(count.get() > 0)` — reactive Rust expression.
    // `@Show(flag)` — bare identifier inside parens, sugar for a signal id.
    if input.peek(Paren) {
        let content;
        syn::parenthesized!(content in input);
        if content.peek(LitStr) {
            return Ok(ShowCond::Signal(content.parse::<LitStr>()?));
        }
        // `|flag|` pipe-sugar inside parens
        if content.peek(Token![|]) {
            content.parse::<Token![|]>()?;
            let ident: Ident = content.parse()?;
            content.parse::<Token![|]>()?;
            return Ok(ShowCond::Signal(LitStr::new(&ident.to_string(), ident.span())));
        }
        // Single identifier inside parens: `@Show(flag)` → signal id
        let fork = content.fork();
        if fork.parse::<Ident>().is_ok() && fork.is_empty() {
            let ident: Ident = content.parse()?;
            return Ok(ShowCond::Signal(LitStr::new(&ident.to_string(), ident.span())));
        }
        return Ok(ShowCond::Rust(content.parse()?));
    }
    // `@Show active` — bare identifier, sugar for a signal id.
    let ident = input.call(Ident::parse_any)?;
    Ok(ShowCond::Signal(LitStr::new(&ident.to_string(), ident.span())))
}

fn parse_block(input: ParseStream) -> Result<Vec<MNode>> {
    let content;
    syn::braced!(content in input);
    parse_children(&content)
}

/// `@else if … { }` chains, or a final `@else { }`.
///
/// `reactive` controls whether an `@else if` link stays a reactive `@Show`
/// (true) or degrades to build-time `@if` (false). Chained conditionals keep
/// the same kind as their head, so `@Show(x) @else if (y) @else` is all-show.
fn parse_else(input: ParseStream, reactive: bool) -> Result<(Vec<MNode>, bool)> {
    // `else` is a keyword, so it must be peeked as a token rather than as an
    // identifier.
    if !(input.peek(Token![@]) && input.peek2(Token![else])) {
        return Ok((Vec::new(), false));
    }
    input.parse::<Token![@]>()?;
    input.parse::<Token![else]>()?;

    // `else if` — re-enter the conditional parser with the keyword consumed.
    if input.peek(Token![if]) {
        input.parse::<Token![if]>()?;
        if reactive {
            return parse_show_if(input).map(|node| (vec![node], true));
        }
        return parse_if(input).map(|node| (vec![node], true));
    }

    let block = parse_block(input)?;
    Ok((block, true))
}

/// `@Show(cond) { … }` parsed with `@else if` already consumed. Mirrors
/// `parse_if` but produces `MNode::Show` so an `@else if` chain stays
/// reactive end to end.
fn parse_show_if(input: ParseStream) -> Result<MNode> {
    let when = parse_show_cond(input)?;
    let then = parse_block(input)?;
    let (otherwise, has_else) = parse_else(input, true)?;
    Ok(MNode::Show {
        when,
        then,
        otherwise,
        has_else,
    })
}

/// Parsed with the `if` keyword already consumed. Build-time `@if`: the
/// condition is a plain Rust expression; `@else if` links stay `@if`.
fn parse_if(input: ParseStream) -> Result<MNode> {
    let test: RustExpr = input.parse()?;
    let then = parse_block(input)?;
    let (otherwise, has_else) = parse_else(input, false)?;
    Ok(MNode::If {
        test,
        then,
        otherwise,
        has_else,
    })
}

fn parse_for(input: ParseStream) -> Result<MNode> {
    let pat = Pat::parse_single(input)?;
    input.parse::<Token![in]>()?;
    let iter: RustExpr = input.parse()?;

    // Optional `, key = t.path`. Read *before* the body, since it must not be
    // mistaken for the first child of the `{ … }`.
    let key = if input.peek(Token![,]) {
        input.parse::<Token![,]>()?;
        let kw = input.call(Ident::parse_any)?.to_string();
        if kw != "key" {
            return Err(syn::Error::new(
                input.span(),
                format!(
                    "vhtml: expected `key = …` after the iterable in @for; `{kw}` is not a loop clause"
                ),
            ));
        }
        input.parse::<Token![=]>()?;
        let base = input.call(Ident::parse_any)?;
        let base_span = base.span();
        let mut steps = Vec::new();
        while input.peek(Token![.]) {
            input.parse::<Token![.]>()?;
            let name = input.call(Ident::parse_any)?.to_string();
            if input.peek(syn::token::Paren) {
                // `t.get("id")` — the accessor spelling of a path step; only
                // `get` with exactly one string literal is a path, anything
                // else is ordinary Rust this grammar does not speak.
                if name != "get" {
                    return Err(syn::Error::new(
                        base_span,
                        "vhtml: a `@for` key is a value path — `t.id` / `t.get(\"id\")` / `index`",
                    ));
                }
                let inner;
                syn::parenthesized!(inner in input);
                let lit = inner.parse::<LitStr>()?;
                if !inner.is_empty() {
                    return Err(syn::Error::new(
                        inner.span(),
                        "vhtml: `t.get(…)` in a @for key takes exactly one field name",
                    ));
                }
                steps.push(lit.value());
            } else {
                steps.push(name);
            }
        }
        Some(KeyPath {
            base: base.to_string(),
            steps,
            base_span,
        })
    } else {
        None
    };

    let body = parse_block(input)?;
    Ok(MNode::For {
        pat: Box::new(pat),
        iter,
        key,
        body,
    })
}

// =============================================================================
// Lowering to `velo_ast`
// =============================================================================

// The row-binding context.
//
// Keyed `@for` bodies are lowered to `Node::Each`, whose rows render from a
// single template — so anything touching the row has to lower to an `Expr`,
// not to a plain Rust closure capturing one row per build. The `Frame` type
// is what lets the macro tell a row read (`t.name`) from an unrelated Rust
// expression (`window.innerWidth`), which is why the two can keep living in
// the same body without the template silently missing the row.

/// `name` bound to a keyed loop frame: `(depth, is_the_index_alias)`.
///
/// Depth is the count of keyed frames *newer* than the one read, which is
/// what the resolver counts: `Expr::LoopItem(0)` is the innermost row, and
/// each enclosing keyed loop shifts an outer read one further out. Shadowing
/// follows Rust's newest-first rule, so unkeyed frames are invisible here —
/// a keyed read has to name a keyed binding.
fn keyed_binding(ctx: &[Frame], name: &str) -> Option<(u16, bool)> {
    let pos = ctx
        .iter()
        .rposition(|f| f.keyed && (f.name == name || f.index.as_deref() == Some(name)))?;
    let is_index = ctx[pos].index.as_deref() == Some(name);
    let depth = ctx[pos + 1..].iter().filter(|f| f.keyed).count() as u16;
    Some((depth, is_index))
}

/// Every variable-shape ident in the expression.
fn vars_in<'e>(expr: &'e RustExpr, out: &mut Vec<&'e Ident>) {
    match expr {
        RustExpr::Path(p) => {
            if p.path.segments.len() == 1 {
                out.push(&p.path.segments.last().unwrap().ident);
            }
        }
        RustExpr::Field(f) => vars_in(&f.base, out),
        RustExpr::Index(i) => {
            vars_in(&i.expr, out);
            vars_in(&i.index, out);
        }
        RustExpr::MethodCall(m) => {
            vars_in(&m.receiver, out);
            m.args.iter().for_each(|a| vars_in(a, out));
        }
        RustExpr::Binary(b) => {
            vars_in(&b.left, out);
            vars_in(&b.right, out);
        }
        RustExpr::Unary(u) => vars_in(&u.expr, out),
        RustExpr::Paren(p) => vars_in(&p.expr, out),
        RustExpr::Group(g) => vars_in(&g.expr, out),
        RustExpr::Tuple(t) => t.elems.iter().for_each(|e| vars_in(e, out)),
        RustExpr::If(i) => {
            vars_in(&i.cond, out);
            block_vars_in(&i.then_branch, out);
            if let Some((_, e)) = &i.else_branch {
                vars_in(e, out);
            }
        }
        _ => {}
    }
}

/// Adds the idents referenced by a plain block.
fn block_vars_in<'b>(block: &'b syn::Block, out: &mut Vec<&'b Ident>) {
    block.stmts.iter().for_each(|s| match s {
        syn::Stmt::Expr(e, _) => vars_in(e, out),
        syn::Stmt::Local(l) => {
            if let Some(init) = &l.init {
                vars_in(&init.expr, out);
            }
        }
        _ => {}
    });
}

/// The value a block reduces to: exactly one trailing expression. A keyed
/// `if`'s branches are Rust blocks, but a `Cond` holds expressions — so the
/// block has to be a single expression statement to have a value at all.
fn block_tail(block: &syn::Block) -> Res<&RustExpr> {
    match block.stmts.as_slice() {
        [syn::Stmt::Expr(e, None)] => Ok(e),
        _ => Err(syn::Error::new_spanned(block, ROW_GRAMMAR)),
    }
}

/// Does the expression even mention a keyed row binding?
///
/// Splices that do not are left as ordinary Rust; only the ones that do have
/// to speak the value grammar. Path shapes are enough: a field chain starts at
/// one, and literals never mention anything.
fn mentions_keyed(expr: &RustExpr, ctx: &[Frame]) -> bool {
    let mut idents = Vec::new();
    vars_in(expr, &mut idents);
    idents.iter().any(|id| keyed_binding(ctx, &id.to_string()).is_some())
}

/// Interpret one row expression to an `Expr` token stream.
///
/// `Ok(None)`: the expression never mentions a keyed row — leave it plain
/// Rust. `Ok(Some(..))`: an `Expr` fragment whose evaluation is the same
/// value, wherever the template re-renders the row. `Err`: it does mention
/// the row and nothing in the grammar covers it — report the whole shape so
/// the author can either rewrite it or drop the key clause.
///
/// The grammar is the intersection of "commonly written in a row body" and
/// "expressible as data": reads, literals, comparisons, arithmetic, boolean
/// logic, ternaries, and `format!` with plain placeholders. Every other shape
/// (a method other than `.get`, any call, …) stays an error, on purpose — the
/// key clause buys the reconciler, and this is its price, stated once here.
fn interpret(expr: &RustExpr, ctx: &[Frame]) -> Res<Option<TokenStream2>> {
    let a = ast();

    match expr {
        // A bare binding: the row itself, or the row index of a `(t, i)` pat.
        RustExpr::Path(p) => {
            let id = p.path.segments.last().unwrap().ident.to_string();
            match keyed_binding(ctx, &id) {
                Some((depth, true)) => Ok(Some(quote!(#a::Expr::index(#depth)))),
                Some((depth, false)) => Ok(Some(quote!(#a::Expr::item(#depth)))),
                None => Ok(None),
            }
        }

        // `t.title` — a field chain over a row read (or a deeper nested row).
        RustExpr::Field(f) => {
            let Some(base) = interpret(&f.base, ctx)? else {
                return Ok(None);
            };
            match &f.member {
                syn::Member::Named(ident) => {
                    let key = ident.to_string();
                    Ok(Some(quote! {
                        #a::Expr::Member {
                            base: ::std::boxed::Box::new(#base),
                            key: ::std::string::String::from(#key),
                        }
                    }))
                }
                syn::Member::Unnamed(_) => Err(syn::Error::new_spanned(
                    &f.member,
                    "vhtml: a keyed row is a map — read fields by name, not position",
                )),
            }
        }

        // `t.get("title")` — the accessor spelling of a field, matching how a
        // `Value` is read in plain Rust today. `t.get(i)` indexes a list.
        RustExpr::MethodCall(m) => {
            let Some(receiver) = interpret(&m.receiver, ctx)? else {
                return Ok(None);
            };
            let args: Vec<&RustExpr> = m.args.iter().collect();
            match (m.method.to_string().as_str(), args.as_slice()) {
                ("get", [RustExpr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. })]) => {
                    let key = s.value();
                    Ok(Some(quote! {
                        #a::Expr::Member {
                            base: ::std::boxed::Box::new(#receiver),
                            key: ::std::string::String::from(#key),
                        }
                    }))
                }
                ("get", [only]) => match interpret(only, ctx)? {
                    Some(key) => Ok(Some(quote! {
                        #a::Expr::Index {
                            base: ::std::boxed::Box::new(#receiver),
                            key: ::std::boxed::Box::new(#key),
                        }
                    })),
                    None => Err(syn::Error::new_spanned(
                        only,
                        "vhtml: `get(…)` on the row takes a string field name",
                    )),
                },
                _ => Err(syn::Error::new_spanned(
                    m,
                    "vhtml: the only method a keyed row understands is `get(\"field\")` — \
                     for anything else, drop the `key = …` clause and use the plain Rust loop",
                )),
            }
        }

        RustExpr::Lit(l) => Ok(Some(lit_expr(&l.lit)?)),

        RustExpr::Paren(p) => interpret(&p.expr, ctx),

        RustExpr::Group(g) => interpret(&g.expr, ctx),

        RustExpr::Block(b) => interpret(block_tail(&b.block)?, ctx),

        RustExpr::Binary(b) => {
            if !mentions_keyed(expr, ctx) {
                return Ok(None);
            }
            let l = required(&b.left, ctx)?;
            let r = required(&b.right, ctx)?;
            match &b.op {
                syn::BinOp::Eq(_) | syn::BinOp::Ne(_) | syn::BinOp::Lt(_)
                | syn::BinOp::Le(_) | syn::BinOp::Gt(_) | syn::BinOp::Ge(_) => {
                    let cmp = cmp_ident(&b.op);
                    Ok(Some(quote! {
                        #a::Expr::Cmp { op: #a::expr::CmpOp::#cmp, l: ::std::boxed::Box::new(#l), r: ::std::boxed::Box::new(#r) }
                    }))
                }
                syn::BinOp::Add(_) | syn::BinOp::Sub(_) | syn::BinOp::Mul(_) | syn::BinOp::Div(_) => {
                    let op = arith_ident(&b.op);
                    Ok(Some(quote! {
                        #a::Expr::Arith { op: #a::expr::ArithOp::#op, l: ::std::boxed::Box::new(#l), r: ::std::boxed::Box::new(#r) }
                    }))
                }
                syn::BinOp::And(_) => Ok(Some(quote!(#a::Expr::And(::std::vec![#l, #r])))),
                syn::BinOp::Or(_) => Ok(Some(quote!(#a::Expr::Or(::std::vec![#l, #r])))),
                _ => Err(syn::Error::new_spanned(b, ROW_GRAMMAR)),
            }
        }

        RustExpr::Unary(u) => {
            if !mentions_keyed(expr, ctx) {
                return Ok(None);
            }
            let inner = required(&u.expr, ctx)?;
            match u.op {
                syn::UnOp::Not(_) => Ok(Some(quote!(#a::Expr::Not(::std::boxed::Box::new(#inner))))),
                syn::UnOp::Neg(_) => Ok(Some(quote!(#a::Expr::Neg(::std::boxed::Box::new(#inner))))),
                _ => Err(syn::Error::new_spanned(u, ROW_GRAMMAR)),
            }
        }

        // Rust's `if` is the ternary here; the AST's `Cond` evaluates exactly
        // one branch. An `if` with no `else` yields `Expr::Missing`.
        RustExpr::If(i) => {
            if !mentions_keyed(expr, ctx) {
                return Ok(None);
            }
            let test = required(&i.cond, ctx)?;
            let then = required(block_tail(&i.then_branch)?, ctx)?;
            let otherwise = match &i.else_branch {
                Some((_, e)) => required(e, ctx)?,
                None => quote!(#a::Expr::Missing),
            };
            Ok(Some(quote! {
                #a::Expr::Cond {
                    test: ::std::boxed::Box::new(#test),
                    then: ::std::boxed::Box::new(#then),
                    otherwise: ::std::boxed::Box::new(#otherwise),
                }
            }))
        }

        // `format!("{} {}", t.a, b)` — the string building of the "keep plain
        // Rust" story, in its most common shape. Lowered to `Expr::Format`,
        // the AST's own templating: literal chunks stay static, placeholders
        // become the inner reads.
        RustExpr::Macro(m) if m.mac.path.is_ident("format") => format_expr(m, ctx),

        other => {
            if mentions_keyed(other, ctx) {
                Err(syn::Error::new_spanned(other, ROW_GRAMMAR))
            } else {
                Ok(None)
            }
        }
    }
}

/// An operand inside a keyed row expression that mentions the row but not in
/// a readable shape is a hard error at that operand.
fn required(expr: &RustExpr, ctx: &[Frame]) -> Res<TokenStream2> {
    match interpret(expr, ctx)? {
        Some(tokens) => Ok(tokens),
        None => Err(syn::Error::new_spanned(expr, ROW_GRAMMAR)),
    }
}

/// `format!(…)` in a keyed body → [`Expr::Format`](velo_ast::Expr::Format).
///
/// Literal chunks stay static and each placeholder becomes the read it
/// interpolated — `{}`/`{1}` consume the following arguments, `{t}` captures
/// a keyed binding inline the way the real macro does. Width/precision specs
/// (`{:.2}`, `{:>5}`) are rejected rather than silently dropped: the AST
/// template has no formatter, so a spec here would change what renders.
///
/// `Ok(None)` when nothing in the template touches the row: a keyed body is
/// allowed plain `format!` over ordinary Rust state.
fn format_expr(
    call: &syn::ExprMacro,
    ctx: &[Frame],
) -> Res<Option<TokenStream2>> {
    let args: Vec<RustExpr> = syn::parse2::<MacroArgs>(call.mac.tokens.clone())?
        .0
        .into_iter()
        .collect();

    let (lit, rest) = match args.split_first() {
        Some((RustExpr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }), rest)) => (s, rest),
        _ => {
            return Err(syn::Error::new_spanned(
                call,
                "vhtml: `format!` in a keyed body starts with a string literal",
            ))
        }
    };

    // Walk the template, splitting it into literal chunks and arguments.
    enum Seg {
        /// A run of literal text.
        Lit(String),
        /// `{}` / `{1}` — the nth argument after the literal.
        Arg(usize),
        /// `{t}` — an inline capture of a keyed binding.
        Inline(String),
    }
    let mut segs: Vec<Seg> = Vec::new();
    let template = lit.value();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                segs.push(Seg::Lit("{".into()));
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                segs.push(Seg::Lit("}".into()));
            }
            '{' => {
                let mut spec = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    spec.push(c);
                }
                if spec.contains(':') {
                    return Err(syn::Error::new_spanned(
                        call,
                        "vhtml: `{:…}` formatting directives are not part of a keyed row's \
                         template — the AST has no formatter; drop the `key = …` clause for \
                         the full `format!`",
                    ));
                }
                let spec = spec.trim().to_string();
                if spec.is_empty() || spec.chars().all(|c| c.is_ascii_digit()) {
                    segs.push(Seg::Arg(spec.parse::<usize>().unwrap_or(0)));
                } else {
                    segs.push(Seg::Inline(spec));
                }
            }
            _ => {
                // Fold into the last literal chunk.
                match segs.last_mut() {
                    Some(Seg::Lit(s)) => s.push(c),
                    _ => segs.push(Seg::Lit(c.to_string())),
                }
            }
        }
    }

    // An inline name that is not a keyed binding is ordinary Rust capture;
    // the whole expression then stays a plain-Rust render.
    if segs.iter().any(|s| matches!(s, Seg::Inline(name) if keyed_binding(ctx, name).is_none())) {
        return Ok(None);
    }

    let a = ast();
    let mut parts: Vec<TokenStream2> = Vec::new();
    let mut chunk = String::new();
    for seg in &segs {
        match seg {
            Seg::Lit(s) => chunk.push_str(s),
            Seg::Arg(n) => {
                let arg = rest.get(*n).ok_or_else(|| {
                    syn::Error::new_spanned(
                        call,
                        "vhtml: this `format!` placeholder has no matching argument",
                    )
                })?;
                if !chunk.is_empty() {
                    parts.push(quote!(#a::Expr::str(#chunk)));
                    chunk = String::new();
                }
                parts.push(required(arg, ctx)?);
            }
            Seg::Inline(name) => {
                let (depth, is_index) = keyed_binding(ctx, name).unwrap();
                if !chunk.is_empty() {
                    parts.push(quote!(#a::Expr::str(#chunk)));
                    chunk = String::new();
                }
                if is_index {
                    parts.push(quote!(#a::Expr::index(#depth)));
                } else {
                    parts.push(quote!(#a::Expr::item(#depth)));
                }
            }
        }
    }
    if parts.is_empty() {
        return Ok(None);
    }
    if !chunk.is_empty() {
        parts.push(quote!(#a::Expr::str(#chunk)));
    }
    Ok(Some(quote! {
        #a::Expr::Format { parts: ::std::vec![ #(#parts),* ], sep: ::std::string::String::new() }
    }))
}

/// The error text every unsupported row shape folds into, pointing the way
/// back to the plain-Rust loop rather than listing what one shape cannot do.
const ROW_GRAMMAR: &str = concat!(
    "vhtml: a keyed `@for` body can only read the row (`t.field`, `t.get(\"field\")`), ",
    "compare, do arithmetic, `format!` with plain placeholders, or branch — ",
    "anything touching the row another way still belongs in an unkeyed `@for`, ",
    "where dropping the `key = …` clause makes the whole body plain Rust again"
);

/// `BinOp` → `CmpOp` ident.
fn cmp_ident(op: &syn::BinOp) -> syn::Ident {
    let name = match op {
        syn::BinOp::Eq(_) => "Eq",
        syn::BinOp::Ne(_) => "Ne",
        syn::BinOp::Lt(_) => "Lt",
        syn::BinOp::Le(_) => "Le",
        syn::BinOp::Gt(_) => "Gt",
        syn::BinOp::Ge(_) => "Ge",
        _ => unreachable!("checked at the match above"),
    };
    proc_macro2::Ident::new(name, proc_macro2::Span::call_site())
}

/// `BinOp` → `ArithOp` ident.
fn arith_ident(op: &syn::BinOp) -> syn::Ident {
    let name = match op {
        syn::BinOp::Add(_) => "Add",
        syn::BinOp::Sub(_) => "Sub",
        syn::BinOp::Mul(_) => "Mul",
        syn::BinOp::Div(_) => "Div",
        _ => unreachable!("checked at the match above"),
    };
    proc_macro2::Ident::new(name, proc_macro2::Span::call_site())
}

/// A literal in row position. `2` is a float on the wire (`Value` is always
/// `f64`), `2 as usize` is how Rust would have written it — the macro folds
/// the difference, because the author meant a count either way.
fn lit_expr(lit: &syn::Lit) -> Res<TokenStream2> {
    let a = ast();
    match lit {
        syn::Lit::Bool(b) => {
            let v = b.value();
            Ok(quote!(#a::Expr::Bool(#v)))
        }
        syn::Lit::Str(s) => {
            let text = s.value();
            Ok(quote!(#a::Expr::str(#text)))
        }
        syn::Lit::Float(f) => Ok(quote!(#a::Expr::num(#f.value()))),
        syn::Lit::Int(i) => {
            let v: f64 = i.base10_parse()?;
            Ok(quote!(#a::Expr::num(#v)))
        }
        _ => Err(syn::Error::new_spanned(
            lit,
            "vhtml: keyed rows read strings, numbers, and bools — this literal has no value form",
        )),
    }
}

/// The path a `vhtml!` expansion uses to reach the AST. A local alias keeps the
/// generated code readable under `cargo expand`.
fn ast() -> TokenStream2 {
    // Through the re-export, not a bare `velo_ast`. A caller should need
    // exactly one dependency — `veloasm` — to author a view; making them
    // declare `velo_ast` separately is a papercut that buys nothing.
    quote!(::veloasm::velo_ast)
}

/// `(expr)` -> a string-producing reactive expression.
fn emit_render(expr: &RustExpr) -> TokenStream2 {
    let a = ast();
    quote! {
        #a::Expr::render(move |__vhr: &dyn #a::expr::Resolver| ::std::format!("{}", #expr))
    }
}

/// A Rust value used as a condition, normalised through `Value::truthy`.
fn emit_test(expr: &RustExpr) -> TokenStream2 {
    let a = ast();
    quote! {
        #a::Expr::test(move |__vhr: &dyn #a::expr::Resolver| -> bool {
            #a::Value::from(#expr).truthy()
        })
    }
}

fn expand_attr(attr: &MAttr, ctx: &[Frame]) -> Res<TokenStream2> {
    let a = ast();
    match (&attr.name, &attr.value) {
        (_, Some(AttrValueSrc::ClassLit(cls))) => {
            let cls = cls.as_str();
            Ok(quote! { #a::Attr::class_toggle(#cls, #a::Expr::Bool(true)) })
        }
        // `class:critical=(cond)` — same toggle, with the condition computed
        // in Rust rather than baked on.
        (_, Some(AttrValueSrc::ClassCond { cls, expr, is_raw })) => {
            let cls = cls.as_str();
            let test = if *is_raw {
                // Raw Expr used directly as the test expression.
                quote! { #expr }
            } else {
                lower_splice(expr, ctx, emit_test)?
            };
            Ok(quote! { #a::Attr::class_toggle(#cls, #test) })
        }
        // An event binding becomes an EventBinding, not an attribute.
        (_, Some(AttrValueSrc::Splice(expr))) if attr.event.is_some() => {
            let event = attr.event.as_ref().unwrap();
            let action = expand_event_action(expr);
            Ok(quote! { #a::EventBinding::new(#event, #action) })
        }
        (name, None) => {
            let name = name.as_str();
            Ok(quote! { #a::Attr::boolean(#name, #a::Expr::Bool(true)) })
        }
        (name, Some(AttrValueSrc::Lit(lit))) => {
            let name = name.as_str();
            let text = lit.value();
            Ok(quote! { #a::Attr::stat(#name, #text) })
        }
        (name, Some(AttrValueSrc::RawExpr(expr))) => {
            // A raw `velo_ast::Expr` used directly as the attribute value.
            let name = name.as_str();
            Ok(quote! { #a::Attr::bound(#name, #expr) })
        }
        (_, Some(AttrValueSrc::DollarExpr(expr))) if attr.event.is_some() => {
            let event = attr.event.as_ref().unwrap();
            let action = expand_event_action(expr);
            Ok(quote! { #a::EventBinding::new(#event, #action) })
        }
        (name, Some(AttrValueSrc::DollarExpr(expr))) => {
            // A raw `velo_ast::Expr` used directly as the attribute value.
            let name = name.as_str();
            Ok(quote! { #a::Attr::bound(#name, #expr) })
        }
        (name, Some(AttrValueSrc::Splice(expr))) => {
            let rendered = lower_splice(expr, ctx, emit_render)?;
            let name = name.as_str();
            Ok(quote! { #a::Attr::bound(#name, #rendered) })
        }
    }
}

/// Expand an event action expression, supporting:
/// - Direct EventAction: `add("count", 1)`
/// - Closure returning EventAction: `|| add("count", 1)`
/// - Closure with multiple ops: `|| { add("count", 1); toggle("flag") }`
fn expand_event_action(expr: &RustExpr) -> TokenStream2 {
    // Check if it's a closure expression (|| { ... } or || expr)
    let is_closure = matches!(expr, RustExpr::Closure(_));

    let helpers = event_helpers();

    if is_closure {
        // For closures, we need to call them to get the EventAction
        // The closure should return EventAction or multiple ops that we combine
        quote! {{
            #helpers
            let __action = (#expr)();
            __action
        }}
    } else {
        // For direct expressions, wrap with helpers in scope
        quote! {{
            #helpers
            (#expr)
        }}
    }
}

/// A splice lowered against the row context.
///
/// Inside a keyed `@for`, an expression that reads the row has to become an
/// `Expr` — the template re-renders rows from data, so a closure capturing
/// one row per build would freeze whatever the *first* row had. Splices that
/// never mention the row keep the plain-Rust path they have outside loops.
/// `emit` is that plain-Rust lowering: `emit_render` for text positions,
/// `emit_test` for conditions.
///
/// Always returns an [`Expr`] token stream — either an interpreted row `Expr`
/// (keyed context) or the plain-Rust `emit` result.
fn lower_splice(
    expr: &RustExpr,
    ctx: &[Frame],
    emit: impl Fn(&RustExpr) -> TokenStream2,
) -> Res<TokenStream2> {
    if !ctx.is_empty() {
        if let Some(tokens) = interpret(expr, ctx)? {
            return Ok(tokens);
        }
    }
    Ok(emit(expr))
}

/// `key = t.id` → the `Expr` the reconciler evaluates per row.
///
/// `index` is the positional identity, spelled as a choice rather than a
/// default. Any other base must name a keyed binding — this loop's own, or an
/// enclosing one — and the field steps after it fold into `Expr::Member`.
fn lower_key(key: &KeyPath, ctx: &[Frame]) -> Res<TokenStream2> {
    let a = ast();
    let base = if key.base == "index" && keyed_binding(ctx, "index").is_none() {
        quote!(#a::Expr::index(0))
    } else {
        match keyed_binding(ctx, &key.base) {
            Some((depth, true)) => quote!(#a::Expr::index(#depth)),
            Some((depth, false)) => quote!(#a::Expr::item(#depth)),
            None => {
                return Err(syn::Error::new(
                    key.base_span,
                    format!(
                        "vhtml: the `@for` key starts at `{}`, which is not a keyed loop binding — \
                         the key path names this loop's binding (or an enclosing keyed one)",
                        key.base
                    ),
                ))
            }
        }
    };
    Ok(key.steps.iter().fold(base, |acc, step| {
        let name = step.clone();
        quote! {
            #a::Expr::Member {
                base: ::std::boxed::Box::new(#acc),
                key: ::std::string::String::from(#name),
            }
        }
    }))
}

/// The binding names a keyed `@for` may read: `t`, or `(t, i)`.
fn row_bindings(pat: &Pat) -> Option<(String, Option<String>)> {
    match pat {
        Pat::Ident(i) => Some((i.ident.to_string(), None)),
        Pat::Tuple(t) => {
            let mut elems = t.elems.iter().filter_map(|p| match p {
                Pat::Ident(i) => Some(i.ident.to_string()),
                _ => None,
            });
            match (elems.next(), elems.next(), elems.next()) {
                (Some(name), index, None) => Some((name, index)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn expand_node(node: &MNode, ctx: &[Frame]) -> Res<TokenStream2> {
    let a = ast();
    match node {
        MNode::Text(lit) => {
            let text = lit.value();
            Ok(quote! { #a::Node::text(#text) })
        }

        MNode::Splice(expr) => {
            let rendered = lower_splice(expr, ctx, emit_render)?;
            Ok(quote! { #a::Node::text_expr(#rendered) })
        }

        MNode::RawExpr(expr) => {
            Ok(quote! { #a::Node::text_expr(#expr) })
        }

        MNode::DollarExpr(expr) => {
            Ok(quote! { #a::Node::text_expr(#expr) })
        }

        MNode::If {
            test,
            then,
            otherwise,
            has_else,
        } => {
            // Build-time in Maud, but inside a keyed body a condition over the
            // row cannot be decided at build time — it lowers to the AST's
            // conditional, same as `@Show`'s Rust form does.
            let cond = lower_splice(test, ctx, emit_test)?;
            let then_tokens = expand_children(then, ctx)?;
            let else_tokens = if *has_else {
                expand_children(otherwise, ctx)?
            } else {
                quote!(#a::Node::Missing)
            };
            Ok(quote! { #a::Node::cond(#cond, #then_tokens, #else_tokens) })
        }

        MNode::Show {
            when,
            then,
            otherwise,
            has_else,
        } => {
            let cond = match when {
                // A string literal is a signal id: portable, and the only form
                // that can ship to a browser as JSON.
                ShowCond::Signal(lit) => {
                    let s = proc_macro2::Literal::string(&lit.value());
                    quote! { #a::Expr::signal(#s) }
                }
                ShowCond::Rust(expr) => lower_splice(expr, ctx, emit_test)?,
            };
            let then_tokens = expand_children(then, ctx)?;
            let fallback = if *has_else {
                expand_children(otherwise, ctx)?
            } else {
                quote!(#a::Node::Missing)
            };
            Ok(quote! { #a::Node::show(#cond, #then_tokens, #fallback) })
        }

        // `@let` renders nothing; it only binds. Hoisted by `expand_children`,
        // which is why a sibling after it can see the binding. Reached from
        // here only if someone nests one where there is no sibling scope to
        // bind for.
        MNode::Let { .. } => Ok(quote!(#a::Node::Missing)),

        // The unkeyed loop body is re-evaluated inside an effect, so a signal
        // read while building the rows (`@for t in threats.get()`) re-runs the
        // whole loop and the list updates in place. This is what keeps Maud's
        // plain-Rust loop body — loop variable, field access, `format!` —
        // instead of forcing it through `Value`.
        //
        // The tradeoff is that the closure must be `'static`: a loop body may
        // only use owned values or signals, not borrow a local that dies with
        // the surrounding function. Rust says so plainly at the offending
        // borrow.
        //
        // The keyed form trades plain Rust for reconciliation: rows become
        // values, the body becomes one template, and moved rows are placed
        // rather than rebuilt. See `interpret` for the row grammar.
        MNode::For { pat, iter, key, body } => match key {
            None => {
                let pat = pat.as_ref();
                let body_tokens = expand_children(body, ctx)?;
                Ok(quote! {
                    #a::Node::native_render(move |__vhr: &dyn #a::expr::Resolver| {
                        let mut __vhl: ::std::vec::Vec<#a::Node> = ::std::vec::Vec::new();
                        for #pat in #iter {
                            __vhl.push(#body_tokens);
                        }
                        #a::Node::fragment(__vhl)
                    })
                })
            }
            Some(key) => {
                let (name, index) =
                    row_bindings(pat).ok_or_else(|| {
                        syn::Error::new_spanned(
                            pat,
                            "vhtml: a keyed `@for` binds `t`, or `(t, i)` for the index — \
                             other patterns have no value form",
                        )
                    })?;

                // Push this loop's frame, then lower the key and the body
                // against it. The key resolves with the frame already in
                // place: its own binding reads as depth 0, exactly as the
                // reconciler's `eval_keyed` sees it.
                let mut ctx_here: Vec<Frame> = ctx.to_vec();
                ctx_here.push(Frame {
                    name,
                    index,
                    keyed: true,
                });

                // An outer loop's rows are data; the runtime evaluates `items`
                // against the *enclosing* shim, before this loop's frame exists.
                // A row-data source (`t.children`) is therefore an `Expr` over
                // `LoopItem`, while a plain Rust source (`threats.get()`) stays
                // a native list closure.
                let items = if mentions_keyed(iter, ctx) {
                    required(iter, ctx)?
                } else {
                    quote!(#a::Expr::native_list(
                        move |__vhr: &dyn #a::expr::Resolver| #iter
                    ))
                };
                let key_tokens = lower_key(key, &ctx_here)?;
                let body_tokens = expand_children(body, &ctx_here)?;
                Ok(quote! { #a::Node::each(#items, #key_tokens, #body_tokens) })
            }
        },

        MNode::Element(el) => {
            let tag = el.tag.as_str();

            // Built-in reactive control-flow: `<Show when=… fallback=…>…</Show>`
            // and `<Suspense loading=… fallback=…>…</Suspense>` (inverted predicate).
            // Lowered directly to [`Node::show`](velo_ast::Node::show) so they
            // render reactively instead of as opaque component nodes.
            if tag == "Show" || tag == "Suspense" {
                return expand_show(el, ctx, tag == "Suspense");
            }

            let mut attrs = Vec::new();
            let mut events = Vec::new();
            for attr in &el.attrs {
                let token = expand_attr(attr, ctx)?;
                if attr.event.is_some() {
                    events.push(token);
                } else {
                    attrs.push(token);
                }
            }
            // Through `expand_children`, not a bare map, so a `@let` among the
            // children is hoisted into the same scope as its siblings. Mapping
            // each child directly gave every one its own block, and a `@let` in
            // an element body silently never bound.
            let children = expand_children(&el.children, ctx)?;

            if el.is_component {
                Ok(quote! {
                    #a::Node::Component {
                        name: #tag.to_string(),
                        attrs: ::std::vec![ #(#attrs),* ],
                        children: ::std::vec![ #children ],
                    }
                })
            } else {
                Ok(quote! {
                    #a::Node::Element(#a::Element {
                        tag: #tag.to_string(),
                        attrs: ::std::vec![ #(#attrs),* ],
                        events: ::std::vec![ #(#events),* ],
                        children: ::std::vec![ #children ],
                    })
                })
            }
        }
    }
}

/// `<Show when=… fallback=…>…</Show>` — reactive conditional, lowered to
/// [`Node::show`](velo_ast::Node::show). `<Suspense loading=…>` inverts the
/// predicate so content shows once loading is done.
fn expand_show(el: &MElement, ctx: &[Frame], is_suspense: bool) -> Res<TokenStream2> {
    let a = ast();
    let mut when: Option<TokenStream2> = None;
    let mut fallback: Option<TokenStream2> = None;

    for attr in &el.attrs {
        match attr.name.as_str() {
            "when" | "loading" => {
                let tokens = match &attr.value {
                    Some(AttrValueSrc::Lit(lit)) => {
                        let id = lit.value();
                        quote! { #a::Expr::signal(#id) }
                    }
                    Some(AttrValueSrc::Splice(expr)) => {
                        lower_splice(expr, ctx, emit_test)?
                    }
                    Some(AttrValueSrc::RawExpr(expr)) => {
                        quote! { #expr }
                    }
                    Some(AttrValueSrc::DollarExpr(expr)) => {
                        quote! { #expr }
                    }
                    _other => {
                        return Err(syn::Error::new_spanned(
                            &attr.name,
                            "Show `when` needs a signal id (`when=\"active\"`), an \
                             expression (`when={ cond }`), or a raw Expr \
                             (`when=${ Expr::signal(\"active\") }`)",
                        ))
                    }
                };
                when = Some(tokens);
            }
            "fallback" => {
                let tokens = match &attr.value {
                    Some(AttrValueSrc::Lit(lit)) => {
                        let text = lit.value();
                        quote! { #a::Node::text(#text) }
                    }
                    Some(AttrValueSrc::Splice(expr)) => {
                        quote! { #expr }
                    }
                    Some(AttrValueSrc::RawExpr(expr)) => {
                        quote! { #expr }
                    }
                    Some(AttrValueSrc::DollarExpr(expr)) => {
                        quote! { #expr }
                    }
                    _other => {
                        return Err(syn::Error::new_spanned(
                            &attr.name,
                            "Show `fallback` needs a static string \
                             (`fallback=\"No\"`), an expression \
                             (`fallback={ Node::text(\"No\") }`), or a raw Expr \
                             (`fallback=${ Node::text(\"No\") }`)",
                        ))
                    }
                };
                fallback = Some(tokens);
            }
            other => {
                return Err(syn::Error::new_spanned(
                    &attr.name,
                    format!(
                        "unknown attribute `{other}` on `<Show>` — only `when`/`loading` \
                         and `fallback` are allowed"
                    ),
                ))
            }
        }
    }

    let when = when.ok_or_else(|| {
        syn::Error::new_spanned(&el.tag, "`<Show>` requires a `when` attribute")
    })?;
    let fallback = fallback.unwrap_or_else(|| quote! { #a::Node::text("") });
    let children = expand_children(&el.children, ctx)?;

    // Suspense inverts: content shows when loading is false.
    let predicate = if is_suspense {
        quote! { #a::Expr::Not(::std::boxed::Box::new(#when)) }
    } else {
        when
    };

    Ok(quote! {
        #a::Node::show(#predicate, #children, #fallback)
    })
}

/// Children become a single node, or a fragment when there are siblings.
///
/// `@let` bindings are hoisted and the whole run is wrapped in one block. This is
/// the whole reason a sibling can see a binding at all: emitting children as a
/// comma-separated fragment gives each `@let` its own `{ … }`, so the binding
/// would never escape it and every use after the declaration would fail to
/// compile. Hoisting costs a block and buys the feature working.
fn expand_children(children: &[MNode], ctx: &[Frame]) -> Res<TokenStream2> {
    let a = ast();

    let mut bindings: Vec<TokenStream2> = Vec::new();
    let mut nodes: Vec<TokenStream2> = Vec::new();
    for child in children {
        match child {
            MNode::Let { name, value } => {
                let value = &**value;
                // A binding over the row cannot hoist: the template is built
                // once, before any row exists.
                if mentions_keyed(value, ctx) {
                    return Err(syn::Error::new_spanned(
                        value,
                        "vhtml: a `@let` inside a keyed `@for` body cannot read the row — \
                         bind constants and locals here; reads belong in the expressions themselves",
                    ));
                }
                bindings.push(quote! { let #name = (#value); });
            }
            other => nodes.push(expand_node(other, ctx)?),
        }
    }

    let body = match nodes.len() {
        0 => quote!(#a::Node::Missing),
        1 => nodes.remove(0),
        _ => quote! { #a::Node::fragment(::std::vec![ #(#nodes),* ]) },
    };

    Ok(if bindings.is_empty() {
        body
    } else {
        quote! {{ #(#bindings)* #body }}
    })
}
/// Expand a token stream into the shared AST.
///
/// Exposed to the crate root so the `#[proc_macro]` entry point can live there,
/// which rustc requires.
/// `MNode` is parsed one node at a time by [`parse_node`]; this shim is what
/// lets a whole `vhtml!` body be read as a comma-separated list.
impl Parse for MNode {
    fn parse(input: ParseStream) -> Result<Self> {
        parse_node(input)
    }
}

/// The whole `vhtml!` input: an optional schema header, then the node list.
struct VhtmlInput {
    schema: Schema,
    nodes: Vec<MNode>,
}

impl Parse for VhtmlInput {
    fn parse(input: ParseStream) -> Result<Self> {
        // The header is only unambiguous when the first word starts something
        // that *cannot* be a node: `schema { … }` would otherwise parse as an
        // element named `schema` and choke on `count = 3` where an attribute
        // list expects a value.
        let schema = if input.peek(Ident::peek_any)
            && input.peek2(syn::token::Brace)
            && input.fork().call(Ident::parse_any)?.to_string() == "schema"
        {
            let schema = parse_schema(input);
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
            Some(schema?)
        } else {
            None
        }
        .unwrap_or(Schema {
            names: Vec::new(),
            present: false,
        });

        let nodes =
            Punctuated::<MNode, Token![,]>::parse_terminated(input)?.into_iter().collect();
        Ok(VhtmlInput { schema, nodes })
    }
}

/// Read the header group: `schema { count = 3.0, hot }`.
///
/// Initial values are recorded and then unused — the schema is an id contract,
/// not a runtime declaration. They are parsed anyway so a form like
/// `count = signal("x")` fails here, where the meaning of the type would be
/// guessable but its grammar wrong, rather than confusingly as code.
fn parse_schema(input: ParseStream) -> Result<Schema> {
    let id: Ident = input.call(Ident::parse_any)?;
    let mut names: Vec<String> = Vec::new();
    let mut errors: Vec<syn::Error> = Vec::new();

    let inner;
    syn::braced!(inner in input);
    while !inner.is_empty() {
        let name = inner.call::<Ident>(Ident::parse_any)?;
        if names.contains(&name.to_string()) {
            errors.push(syn::Error::new(
                name.span(),
                format!("vhtml: signal `{}` is declared twice in the schema", name),
            ));
        }
        names.push(name.to_string());
        if inner.peek(Token![=]) {
            inner.parse::<Token![=]>()?;
            let value: RustExpr = inner.parse()?;
            match value {
                RustExpr::Lit(_) => {}
                other => errors.push(syn::Error::new_spanned(
                    other,
                    "vhtml: a schema initial value must be a literal (it is a contract, not a runtime)",
                )),
            }
        }
        if !inner.is_empty() {
            inner.parse::<Token![,]>()?;
        }
    }

    if names.is_empty() {
        errors.push(syn::Error::new(
            id.span(),
            "vhtml: `schema { }` declares nothing — list the signals the island may name, or drop the header",
        ));
    }
    if let Some(e) = errors.into_iter().reduce(|a, b| {
        syn::Error::new(a.span(), format!("{}\n{}", a, b))
    }) {
        return Err(e);
    }
    Ok(Schema {
        names,
        present: true,
    })
}

/// Every string-literal signal id the tree names, with where it was named.
fn collect_signal_uses<'a>(nodes: &'a [MNode], out: &mut Vec<&'a LitStr>) {
    for node in nodes {
        match node {
            MNode::Show { when, then, otherwise, .. } => {
                if let ShowCond::Signal(lit) = when {
                    out.push(lit);
                }
                collect_signal_uses(then, out);
                collect_signal_uses(otherwise, out);
            }
            MNode::If { then, otherwise, .. } => {
                collect_signal_uses(then, out);
                collect_signal_uses(otherwise, out);
            }
            MNode::For { body, .. } => collect_signal_uses(body, out),
            MNode::Element(el) => collect_signal_uses(&el.children, out),
            _ => {}
        }
    }
}

/// The compile-time contract, only when a schema header was written.
fn validate_schema(schema: &Schema, nodes: &[MNode]) -> Vec<syn::Error> {
    if !schema.present {
        return Vec::new();
    }
    let mut uses = Vec::new();
    collect_signal_uses(nodes, &mut uses);
    uses.iter()
        .filter(|lit| !schema.names.contains(&lit.value()))
        .map(|lit| {
            syn::Error::new(
                lit.span(),
                format!(
                    "vhtml: signal `{}` is not in the island schema — declare it in the `schema {{ … }}` header",
                    lit.value()
                ),
            )
        })
        .collect()
}

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let body = parse_macro_input!(input as VhtmlInput);
    let errors = validate_schema(&body.schema, &body.nodes);
    if !errors.is_empty() {
        // Report every rejected reference at once; the tree lowers to nothing.
        return errors
            .into_iter()
            .map(|e| e.to_compile_error())
            .collect::<TokenStream2>()
            .into();
    }

    match expand_children(&body.nodes, &[]) {
        Ok(tokens) => TokenStream::from(tokens),
        Err(e) => e.to_compile_error().into(),
    }
}
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
//!     div class="panel" {
//!         h2 { "Live threats" }
//!         @Show("threats") {
//!             ul {
//!                 @for t in threats {
//!                     li .critical { t.get("title") }
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
//! | `@let y = …;` | a Rust `let` | lexical only |
//! | `.cls` | `Attr::class_toggle` | Maud's class shorthand |
//! | `input type="text"` | void element | no block needed |
//! | `// comment` | dropped | Maud line comments |
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
//! Rows are rebuilt rather than reconciled, so there is no DOM-node reuse
//! across list updates. That tradeoff is deliberate: reuse requires comparing
//! row *values*, and a row here is an arbitrary Rust type, not a comparable
//! `Value`. [`Node::each`](velo_ast::Node::each) is the portable, keyed,
//! reconciling loop — the one a backend ships as data, and the one to reach for
//! on a large list.
//!
//! ## Portability
//!
//! Every `(expr)` splice becomes an [`Expr::Native`](velo_ast::Expr::Native),
//! and every `@for` becomes a [`Node::Native`](velo_ast::Node::Native) Rust
//! closure. Neither **can be serialized**, so a `vhtml!` tree is client-only.
//! To build a view a *backend* can ship, use the `velo_ast` builders directly —
//! that is the other half of this feature, and the half that crosses the wire.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream, Result};
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, Expr as RustExpr, Ident, LitStr, Pat, Token};

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
    /// `.cls` — Maud's class shorthand, a statically-on toggle.
    ClassLit(String),
    /// `class:cls=(expr)` — a conditional class toggle.
    ClassCond { cls: String, expr: RustExpr },
}

/// A parsed node.
enum MNode {
    /// `"text"`
    Text(LitStr),
    /// `(expr)`, or a bare expression, in text position.
    Splice(RustExpr),
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
        /// The body, evaluated once per item.
        body: Vec<MNode>,
    },
}

/// The condition of an `@Show`.
enum ShowCond {
    /// `@Show("count")` — a portable signal read.
    Signal(String),
    /// `@Show(cond)` or `@Show(move || cond)` — a Rust test.
    Rust(RustExpr),
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

        let first = input.call(Ident::parse_any)?.to_string();

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

/// `= "literal"` or `= (expr)`.
fn parse_attr_rhs(input: ParseStream) -> Result<AttrValueSrc> {
    if input.peek(syn::token::Brace) {
        let content;
        syn::braced!(content in input);
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
    while !input.is_empty() && !input.peek(syn::token::Brace) {
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
                let (otherwise, has_else) = parse_else(input)?;
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

fn parse_show_cond(input: ParseStream) -> Result<ShowCond> {
    let content;
    syn::parenthesized!(content in input);
    if content.peek(LitStr) {
        return Ok(ShowCond::Signal(content.parse::<LitStr>()?.value()));
    }
    Ok(ShowCond::Rust(content.parse()?))
}

fn parse_block(input: ParseStream) -> Result<Vec<MNode>> {
    let content;
    syn::braced!(content in input);
    parse_children(&content)
}

/// `@else if … { }` chains, or a final `@else { }`.
fn parse_else(input: ParseStream) -> Result<(Vec<MNode>, bool)> {
    // `else` is a keyword, so it must be peeked as a token rather than as an
    // identifier.
    if !(input.peek(Token![@]) && input.peek2(Token![else])) {
        return Ok((Vec::new(), false));
    }
    input.parse::<Token![@]>()?;
    input.parse::<Token![else]>()?;

    // `else if` — re-enter the `@if` parser with the keyword already consumed.
    if input.peek(Token![if]) {
        input.parse::<Token![if]>()?;
        return parse_if(input).map(|node| (vec![node], true));
    }

    let block = parse_block(input)?;
    Ok((block, true))
}

/// Parsed with the `if` keyword already consumed.
fn parse_if(input: ParseStream) -> Result<MNode> {
    let test: RustExpr = input.parse()?;
    let then = parse_block(input)?;
    let (otherwise, has_else) = parse_else(input)?;
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
    let body = parse_block(input)?;
    Ok(MNode::For {
        pat: Box::new(pat),
        iter,
        body,
    })
}

// =============================================================================
// Lowering to `velo_ast`
// =============================================================================

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

fn expand_attr(attr: &MAttr) -> TokenStream2 {
    let a = ast();
    match (&attr.name, &attr.value) {
        (_, Some(AttrValueSrc::ClassLit(cls))) => {
            let cls = cls.as_str();
            quote! { #a::Attr::class_toggle(#cls, #a::Expr::Bool(true)) }
        }
        // `class:critical=(cond)` — same toggle, with the condition computed
        // in Rust rather than baked on.
        (_, Some(AttrValueSrc::ClassCond { cls, expr })) => {
            let cls = cls.as_str();
            let test = emit_test(expr);
            quote! { #a::Attr::class_toggle(#cls, #test) }
        }
        // An event binding becomes an EventBinding, not an attribute.
        (_, Some(AttrValueSrc::Splice(expr))) if attr.event.is_some() => {
            let event = attr.event.as_ref().unwrap();
            quote! { #a::EventBinding::new(#event, (#expr)) }
        }
        (name, None) => {
            let name = name.as_str();
            quote! { #a::Attr::boolean(#name, #a::Expr::Bool(true)) }
        }
        (name, Some(AttrValueSrc::Lit(lit))) => {
            let name = name.as_str();
            let text = lit.value();
            if name == "class" {
                quote! { #a::Attr::stat("class", #text) }
            } else {
                quote! { #a::Attr::stat(#name, #text) }
            }
        }
        (name, Some(AttrValueSrc::Splice(expr))) => {
            let rendered = emit_render(expr);
            let name = name.as_str();
            quote! { #a::Attr::bound(#name, #rendered) }
        }
    }
}

fn expand_node(node: &MNode) -> TokenStream2 {
    let a = ast();
    match node {
        MNode::Text(lit) => {
            let text = lit.value();
            quote! { #a::Node::text(#text) }
        }

        MNode::Splice(expr) => {
            let rendered = emit_render(expr);
            quote! { #a::Node::text(#rendered) }
        }


        MNode::If {
            test,
            then,
            otherwise,
            has_else,
        } => {
            let cond = emit_test(test);
            let then_tokens = expand_children(then);
            let else_tokens = if *has_else {
                expand_children(otherwise)
            } else {
                quote!(#a::Node::Missing)
            };
            quote! { #a::Node::cond(#cond, #then_tokens, #else_tokens) }
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
                ShowCond::Signal(id) => quote! { #a::Expr::signal(#id) },
                ShowCond::Rust(expr) => emit_test(expr),
            };
            let then_tokens = expand_children(then);
            let fallback = if *has_else {
                expand_children(otherwise)
            } else {
                quote!(#a::Node::Missing)
            };
            quote! { #a::Node::show(#cond, #then_tokens, #fallback) }
        }

        // The loop body is re-evaluated inside an effect, so a signal read while
        // building the rows (`@for t in threats.get()`) re-runs the whole loop
        // and the list updates in place. This is what keeps Maud's plain-Rust
        // loop body — loop variable, field access, `format!` — instead of
        // forcing it through `Value`.
        //
        // The tradeoff is that the closure must be `'static`: a loop body may
        // only use owned values or signals, not borrow a local that dies with
        // the surrounding function. Rust says so plainly at the offending
        // borrow. For a portable, keyed, reconciling loop use `Node::each`.
        // `@let` is handled by `expand_children`, which hoists it so the binding
        // reaches the siblings after it. Reached from here only if someone nests
        // one where there is no sibling scope to bind for.
        MNode::Let { .. } => quote!(#a::Node::Missing),

        MNode::For { pat, iter, body } => {
            let pat = pat.as_ref();
            let body_tokens = expand_children(body);
            quote! {
                #a::Node::native_render(move |__vhr: &dyn #a::expr::Resolver| {
                    let mut __vhl: ::std::vec::Vec<#a::Node> = ::std::vec::Vec::new();
                    for #pat in #iter {
                        __vhl.push(#body_tokens);
                    }
                    #a::Node::fragment(__vhl)
                })
            }
        }

        MNode::Element(el) => {
            let tag = el.tag.as_str();
            let mut attrs = Vec::new();
            let mut events = Vec::new();
            for attr in &el.attrs {
                let token = expand_attr(attr);
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
            let children = expand_children(&el.children);

            if el.is_component {
                quote! {
                    #a::Node::Component {
                        name: #tag.to_string(),
                        attrs: ::std::vec![ #(#attrs),* ],
                        children: ::std::vec![ #children ],
                    }
                }
            } else {
                quote! {
                    #a::Node::Element(#a::Element {
                        tag: #tag.to_string(),
                        attrs: ::std::vec![ #(#attrs),* ],
                        events: ::std::vec![ #(#events),* ],
                        children: ::std::vec![ #children ],
                    })
                }
            }
        }
    }
}

/// Children become a single node, or a fragment when there are siblings.
///
/// `@let` bindings are hoisted and the whole run is wrapped in one block. This is
/// the whole reason a sibling can see a binding at all: emitting children as a
/// comma-separated fragment gives each `@let` its own `{ … }`, so the binding
/// would never escape it and every use after the declaration would fail to
/// compile. Hoisting costs a block and buys the feature working.
fn expand_children(children: &[MNode]) -> TokenStream2 {
    let a = ast();

    let mut bindings: Vec<TokenStream2> = Vec::new();
    let mut nodes: Vec<TokenStream2> = Vec::new();
    for child in children {
        match child {
            MNode::Let { name, value } => {
                let value = &**value;
                // `@let` renders nothing; it only binds.
                bindings.push(quote! { let #name = (#value); });
            }
            other => nodes.push(expand_node(other)),
        }
    }

    let body = match nodes.len() {
        0 => quote!(#a::Node::Missing),
        1 => nodes.remove(0),
        _ => quote! { #a::Node::fragment(::std::vec![ #(#nodes),* ]) },
    };

    if bindings.is_empty() {
        body
    } else {
        quote! {{ #(#bindings)* #body }}
    }
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

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let nodes: Vec<MNode> = parse_macro_input!(input with Punctuated::<MNode, Token![,]>::parse_terminated)
        .into_iter()
        .collect();
    TokenStream::from(expand_children(&nodes))
}
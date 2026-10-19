use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use std::path::{Path, PathBuf};
use syn::ext::IdentExt;
use syn::parse::discouraged::Speculative;
use syn::parse::{Parse, ParseStream, Result};
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, Expr, Ident, LitStr, Pat, Token};

/// `vhtml!` — Maud's syntax, compiled to the shared `velo_ast` view tree.
///
/// Lives in its own module because it is a self-contained front end; re-exported
/// here so the proc macro sits at the crate root.
mod vhtml;

/// `vhtml! { … }` — Maud's template syntax, lowered to the shared `velo_ast`
/// view tree instead of to HTML text.
///
/// The implementation lives in [`vhtml`]; the entry point must be here, because
/// rustc requires a `#[proc_macro]` function to sit at the crate root.
///
/// ```ignore
/// use velo_ast::View;
///
/// let root = vhtml! {
///     div class="card" {
///         h2 { "Threats" }
///         @for t in threats { li { t.get("title") } }
///     }
/// };
/// let view = View::new("card", root);
/// ```
#[proc_macro]
pub fn vhtml(input: TokenStream) -> TokenStream {
    vhtml::expand(input)
}

// --- JS-style arrow-closure sugar: `(pat, ..) => expr` / `(pat, ..) => { .. }` ---
// Supports both sync `() => expr` and async `async () => { ... }` forms.
// The `async` keyword gives the Next.js-style async feel for event handlers
// and reactive bodies.
struct ArrowClosure {
    asyncness: Option<Token![async]>,
    inputs: Punctuated<Pat, Token![,]>,
    body: Box<Expr>,
}

impl Parse for ArrowClosure {
    fn parse(input: ParseStream) -> Result<Self> {
        let asyncness = input.parse::<Token![async]>().ok();

        let params;
        syn::parenthesized!(params in input);
        let inputs = params.parse_terminated(Pat::parse_single, Token![,])?;
        input.parse::<Token![=>]>()?;

        let body: Expr = if input.peek(syn::token::Brace) {
            let block: syn::Block = input.parse()?;
            Expr::Block(syn::ExprBlock {
                attrs: vec![],
                label: None,
                block,
            })
        } else {
            input.parse()?
        };

        // Arrow sugar always binds one full expression/block — no trailing
        // tokens should remain in `input` for this attribute/block position.
        Ok(ArrowClosure {
            asyncness,
            inputs,
            body: Box::new(body),
        })
    }
}

/// Speculatively parses `(pat, ..) => ..` sugar. Consumes nothing and
/// returns `None` if the shape doesn't match, so the caller can fall back to
/// normal `Expr` parsing on the untouched stream.
fn try_parse_arrow(input: ParseStream) -> Option<ArrowClosure> {
    let fork = input.fork();
    match fork.parse::<ArrowClosure>() {
        Ok(arrow) => {
            input.advance_to(&fork);
            Some(arrow)
        }
        Err(_) => None,
    }
}

/// Attribute-value position for `on:*` handlers, which must satisfy
/// `FnMut(web_sys::Event) + 'static`. `() => ..` gets an injected, discarded
/// event param; `(e) => ..` binds the event to `e` (untyped — inferred from
/// the `on()` bound, exactly like a hand-written closure would be).
/// Supports `async () => { ... }` for async event handlers (dispatched via
/// wasm-bindgen-futures).
fn parse_event_handler_value(input: ParseStream) -> Result<Expr> {
    if let Some(ArrowClosure { asyncness, inputs, body }) = try_parse_arrow(input) {
        let closure: Expr = if asyncness.is_some() {
            // Async handler: spawn a future via wasm-bindgen-futures
            if inputs.is_empty() {
                syn::parse_quote! { move |_evt: web_sys::Event| {
                    wasm_bindgen_futures::spawn_local(async move { #body })
                } }
            } else {
                syn::parse_quote! { move |#inputs| {
                    wasm_bindgen_futures::spawn_local(async move { #body })
                } }
            }
        } else {
            // Sync handler (existing behavior)
            if inputs.is_empty() {
                syn::parse_quote! { move |_evt: web_sys::Event| #body }
            } else {
                syn::parse_quote! { move |#inputs| #body }
            }
        };
        return Ok(closure);
    }
    input.parse::<Expr>()
}

/// `{ .. }` reactive-expression position, which wants `FnMut() -> R` (see the
/// `Expr::Closure(_)` branch in `ToTokens for VNode` below). Only the 0-arg
/// form makes sense here — reactive expressions don't receive anything — so
/// non-empty params are left unhandled and fall through to a normal parse
/// error rather than silently doing the wrong thing.
/// Supports `async () => { ... }` for async reactive expressions.
fn try_parse_reactive_arrow(input: ParseStream) -> Option<Expr> {
    let fork = input.fork();
    let arrow = fork.parse::<ArrowClosure>().ok()?;
    if !arrow.inputs.is_empty() {
        return None;
    }
    input.advance_to(&fork);
    let body = arrow.body;
    if arrow.asyncness.is_some() {
        // Async reactive expression: return a future
        Some(syn::parse_quote! { move || async move { #body } })
    } else {
        Some(syn::parse_quote! { move || #body })
    }
}

/// Parse one `-segment` of a hyphenated attribute name (`stroke-linecap`,
/// `class:bg-rose-500`, `class:ring-2`, `style:font-size`). Segments may be
/// identifiers or numbers — CSS utility classes are full of numeric parts.
/// Anything else is a parse error.
fn parse_dash_segment(input: ParseStream) -> Result<String> {
    input.parse::<Token![-]>()?;
    // Peek via a fork: `LitInt` also matches a leading-`0x` or suffixed
    // literal (`2xl`), whose exact spelling `to_string()` preserves so the
    // class name round-trips character-for-character.
    let fork = input.fork();
    if fork.parse::<syn::LitInt>().is_ok() {
        Ok(input.parse::<syn::LitInt>()?.to_string())
    } else {
        Ok(input.call(syn::Ident::parse_any)?.to_string())
    }
}

/// In reactive prop positions (`when=`, `style:`, `class:active=`, generic
/// attribute `x={ .. }`) the macro feeds the value through `signal_value!` so a
/// raw `RwSignal<bool>` / `Memo<_>` auto-unwraps. A user-supplied reactive
/// *closure* (`when={ move || bool }`) has no `ViewValue` impl and must instead
/// be CALLED on each effect run. Returns the token stream that evaluates `v` to
/// its current value either way.
fn reactive_value(v: &syn::Expr) -> proc_macro2::TokenStream {
    if matches!(v, syn::Expr::Closure(_)) {
        quote! { (#v)() }
    } else {
        quote! { veloasm::signal_value!(#v) }
    }
}

// A proc-macro AST is built transiently at compile time, so minimizing the
// in-memory size of variants is not worth contorting the AST (the large `Expr`
// and recursive `VNode` fields make the enum inherently big).
#[allow(clippy::large_enum_variant)]
enum VNode {
    Element {
        tag_name: String,
        attributes: Vec<VAttr>,
        children: Vec<VNode>,
    },
    StaticText(String),
    ReactiveExpression(Expr),
    ForLoop {
        pat: Box<syn::Pat>,
        expr: Expr,
        key: Option<Expr>,
        body: Vec<VNode>,
    },
    Fragment(Vec<VNode>), // hold sibling nodes without a wrapper tag!
}

struct VAttr {
    key: String,
    value: Expr,
}

impl ToTokens for VAttr {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        let key = &self.key;
        let value = &self.value;
        tokens.extend(quote! {
            #key: #value
        });
    }
}

impl Parse for VAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key_ident = input.call(syn::Ident::parse_any)?;

        input.parse::<syn::Token![=]>()?;
        let value = input.parse::<syn::Expr>()?;

        let key = key_ident.to_string().trim_start_matches("r#").to_string();

        Ok(VAttr { key, value })
    }
}

// --- Parsing Implementation ---

impl Parse for VNode {
    fn parse(input: ParseStream) -> Result<Self> {
        // Handle expression blocks e.g., { count } or complex conditional blocks
        if input.peek(syn::token::Brace) {
            let fork = input.fork(); // Create a lookahead fork to test loop vs expression blocks safely
            let content;
            syn::braced!(content in fork);

            // Check for inline loop syntax
            if content.peek(Token![for]) {
                let content;
                syn::braced!(content in input); // Advance actual stream pointer
                content.parse::<Token![for]>()?;
                let pat = syn::Pat::parse_single(&content)?;
                content.parse::<Token![in]>()?;
                let expr = content.parse::<Expr>()?;

                // Optional key expression: `key = |item| item.id` or `key = item.id`
                let key = if content.peek(Ident) && content.peek2(Token![=]) {
                    let ident: Ident = content.parse()?;
                    if ident == "key" {
                        content.parse::<Token![=]>()?;
                        Some(content.parse::<Expr>()?)
                    } else {
                        return Err(syn::Error::new(
                            ident.span(),
                            "veloasm: unexpected token after `in <expr>` in for-loop (expected `key =` or `{`)",
                        ));
                    }
                } else {
                    None
                };

                let loop_body_content;
                syn::braced!(loop_body_content in content);

                let mut body = Vec::new();
                while !loop_body_content.is_empty() {
                    body.push(loop_body_content.parse::<VNode>()?);
                }

                return Ok(VNode::ForLoop {
                    pat: Box::new(pat),
                    expr,
                    key,
                    body,
                });
            }

            // Try to parse as a simple expression first, fallback to a full statement block expression!
            let content;
            let braced_token = syn::braced!(content in input); // Advance actual stream pointer

            // `{ () => expr }` sugar for a zero-arg reactive closure, e.g.
            // `{ () => count.get() * 2 }` instead of `{ move || count.get() * 2 }`.
            if let Some(closure_expr) = try_parse_reactive_arrow(&content) {
                if content.is_empty() {
                    return Ok(VNode::ReactiveExpression(closure_expr));
                }
            }

            if let Ok(expr) = content.parse::<Expr>() {
                if content.is_empty() {
                    return Ok(VNode::ReactiveExpression(expr));
                }
            }

            // If it contains statements or semicolons, parse it as a full block statement expression!
            let block = content.call(syn::Block::parse_within)?;
            let expr_block = Expr::Block(syn::ExprBlock {
                attrs: vec![],
                label: None,
                block: syn::Block {
                    brace_token: braced_token,
                    stmts: block,
                },
            });

            return Ok(VNode::ReactiveExpression(expr_block));
        }

        // Handle text literals e.g., "Click me"
        if input.peek(LitStr) {
            let lit: LitStr = input.parse()?;
            return Ok(VNode::StaticText(lit.value()));
        }

        // Parse HTML Tags or Fragments e.g., <div ...> ... </div> or <> ... </>
        input.parse::<Token![<]>()?;

        // Detect Fragment Syntax: Check if the token immediately following `<` is `>`
        if input.peek(Token![>]) {
            input.parse::<Token![>]>()?; // Consume the opening `>`

            let mut children = Vec::new();
            // Parse internal children until we see the start of the closing tag `</`
            while !input.peek(Token![<]) || !input.peek2(Token![/]) {
                children.push(input.parse::<VNode>()?);
            }

            // Consume the fragment closing syntax `</>`
            input.parse::<Token![<]>()?;
            input.parse::<Token![/]>()?;
            input.parse::<Token![>]>()?;

            return Ok(VNode::Fragment(children));
        }

        // Otherwise, proceed to parse standard HTML tag elements
        let tag_ident = input.call(syn::Ident::parse_any)?; // 🚀 Use parse_any for custom component keywords too
        let tag_name = tag_ident.to_string().trim_start_matches("r#").to_string();

        let mut attributes = Vec::new();
        while !input.peek(Token![>]) && !input.peek(Token![/]) {
            // Use parse_any to allow parsing keywords if they show up as attribute parts
            let mut key = input.call(syn::Ident::parse_any)?.to_string();

            // Loop to catch hyphens (e.g., stroke-linecap or aria-describedby).
            // Segments may be numeric (`ring-2` style toggles hit this path
            // after the `class:`/`style:` split below).
            while input.peek(Token![-]) {
                key = format!("{}-{}", key, parse_dash_segment(input)?);
            }

            // Keep your existing namespace colon tracking (e.g., on:click)
            if input.peek(Token![:]) {
                input.parse::<Token![:]>()?;
                let mut sub_key = input.call(syn::Ident::parse_any)?.to_string();
                // Loop to catch hyphens after the colon too
                // (e.g., class:log-row-selected, class:bg-rose-500).
                while input.peek(Token![-]) {
                    sub_key = format!("{}-{}", sub_key, parse_dash_segment(input)?);
                }
                key = format!("{}:{}", key, sub_key);
            }

            // Value-less boolean attribute (`<input disabled />`, `<Link prefetch />`)
            // parses as `key = true`; otherwise read `key = value`.
            let value: Expr = if input.peek(Token![=]) {
                input.parse::<Token![=]>()?;
                if input.peek(syn::token::Brace) {
                    let content;
                    syn::braced!(content in input);
                    if key.starts_with("on:") {
                        // `on:click={() => set_count.update(|c| c + 1)}` sugar,
                        // expanding to `move |_evt: web_sys::Event| ..` / `move |e| ..`.
                        parse_event_handler_value(&content)?
                    } else if content.peek(Token![<]) {
                        // JSX-in-props (React-style): `fallback={ <p>…</p> }`.
                        // No Rust expression can start with `<`, so this is
                        // unambiguous. A single node is used as-is; siblings
                        // are wrapped in a fragment.
                        let mut nodes = Vec::new();
                        while !content.is_empty() {
                            nodes.push(content.parse::<VNode>()?);
                        }
                        let node = if nodes.len() == 1 {
                            nodes.into_iter().next().unwrap()
                        } else {
                            VNode::Fragment(nodes)
                        };
                        syn::parse2(node.to_token_stream())?
                    } else {
                        content.parse()?
                    }
                } else {
                    let lit: LitStr = input.parse()?;
                    // Friendly DX: a string literal where a toggle-bool or a
                    // handler belongs is never intentional, and would otherwise
                    // fail 200 lines deep in generated code (e.g. E0271 bool
                    // vs &str for `class:on="user-card"`). Fail here instead,
                    // pointing at the literal.
                    if key.starts_with("class:") {
                        let want = key.strip_prefix("class:").unwrap();
                        return Err(syn::Error::new(
                            lit.span(),
                            format!(
                                "Velo: `class:{want}=\"...\"` is not valid — `class:` toggles take a boolean (e.g. `class:{want}={{ is_on }}`); for a static class use plain `class=\"...\"`"
                            ),
                        ));
                    }
                    if key.starts_with("on:") {
                        return Err(syn::Error::new(
                            lit.span(),
                            format!(
                                "Velo: `{key}=\"...\"` is not valid — event handlers take a closure (e.g. `{key}={{ move |_| ... }}`)"
                            ),
                        ));
                    }
                    Expr::Lit(syn::ExprLit {
                        attrs: vec![],
                        lit: syn::Lit::Str(lit),
                    })
                }
            } else {
                Expr::Lit(syn::ExprLit {
                    attrs: vec![],
                    lit: syn::Lit::Bool(syn::LitBool {
                        value: true,
                        span: proc_macro2::Span::call_site(),
                    }),
                })
            };

            attributes.push(VAttr { key, value });
        }

        if input.peek(Token![/]) {
            input.parse::<Token![/]>()?;
            input.parse::<Token![>]>()?;
            return Ok(VNode::Element {
                tag_name,
                attributes,
                children: vec![],
            });
        }

        input.parse::<Token![>]>()?;

        let mut children = Vec::new();
        while !input.peek(Token![<]) || !input.peek2(Token![/]) {
            children.push(input.parse::<VNode>()?);
        }

        input.parse::<Token![<]>()?;
        input.parse::<Token![/]>()?;
        let end_tag = input.call(syn::Ident::parse_any)?;
        let end_tag_name = end_tag.to_string().trim_start_matches("r#").to_string();
        if end_tag_name != tag_name {
            return Err(syn::Error::new(
                end_tag.span(),
                format!(
                    "veloasm Macro Error: Expected matching closing tag </{}>",
                    tag_name
                ),
            ));
        }
        input.parse::<Token![>]>()?;

        Ok(VNode::Element {
            tag_name,
            attributes,
            children,
        })
    }
}


impl ToTokens for VNode {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        match self {
            VNode::StaticText(txt) => {
                tokens.extend(quote! {
                    veloasm::DomNode::text(#txt)
                });
            }
            VNode::ReactiveExpression(expr) => {
                match expr {
                    Expr::Closure(_) => {
                        tokens.extend(quote! {
                            veloasm::DomNode::render_expression(#expr)
                        });
                    }
                    _ => {
                        tokens.extend(quote! {
                            veloasm::DomNode::render_expression(move || veloasm::signal_value!(#expr))
                        });
                    }
                }
            }
            VNode::ForLoop {
                pat,
                expr,
                key,
                body,
            } => {
                let compiled_body_nodes = body.iter().map(|child| {
                    quote! { #child }
                });

                if let Some(key_expr) = key {
                    // Keyed, fine-grained list: uses SignalVec + the DOM reconciler.
                    tokens.extend(quote! {
                        {
                            let loop_fragment = veloasm::DomNode::element("div");
                            loop_fragment.set_attr("class", "contents");

                            let list = (#expr).clone();
                            let key_fn = #key_expr;
                            loop_fragment.render_signal_vec(
                                &list,
                                key_fn,
                                move |#pat| -> veloasm::DomNode {
                                    #(#compiled_body_nodes)*
                                }
                            );
                            loop_fragment
                        }
                    });
                } else {
                    tokens.extend(quote! {
                        {
                            let loop_fragment = veloasm::DomNode::fragment();

                            for #pat in #expr {
                                #(
                                    loop_fragment.append(&#compiled_body_nodes);
                                )*
                            }
                            loop_fragment
                        }
                    });
                }
            }
            VNode::Fragment(children) => {
                let compiled_children = children.iter().map(|child| {
                    quote! { #child }
                });

                tokens.extend(quote! {
                    {
                        let fragment_node = veloasm::DomNode::fragment();
                        #(
                            fragment_node.append(&#compiled_children);
                        )*
                        fragment_node
                    }
                });
            }
            VNode::Element {
                tag_name,
                attributes,
                children,
            } => {
                let first_char = tag_name.chars().next().unwrap_or(' ');

                if first_char.is_uppercase() {
                    let component_ident = syn::Ident::new(tag_name, proc_macro2::Span::call_site());
                    let component_name = component_ident.to_string();

                    // `Show` and `Suspense` are reactive control-flow components.
                    //
                    // Unlike ordinary components (whose children are built once at
                    // view!-construction time), these react to a boolean condition
                    // (an async resource's `loading`) and swap between a fallback
                    // and the content:
                    //   <Show when={ !r.loading() } fallback={...}>{ ... }</Show>
                    //   <Suspense loading={ r.loading() } fallback={...}>{ ... }</Suspense>
                    //
                    // Both branches are built ONCE (children are independently
                    // reactive via their own render_expression effects), then
                    // the condition is wrapped in `signal_value!` so raw signals
                    // auto-unwrap into their current value.
                    // `reactive_switch` moves whichever branch is active into a
                    // fragment, swapping live when the condition flips.
                    if component_name == "Show" || component_name == "Suspense" {
                        let mut when_expr: Option<&syn::Expr> = None;
                        let mut fallback_val = quote! { veloasm::DomNode::text("") };
                        for attr in attributes {
                            if attr.key == "when" || attr.key == "loading" {
                                when_expr = Some(&attr.value);
                            } else if attr.key == "fallback" {
                                let v = &attr.value;
                                fallback_val = quote! { #v };
                            }
                        }
                        let content_block = quote! {
                            {
                                let __velo_cf_frag = veloasm::DomNode::fragment();
                                #( __velo_cf_frag.append(&#children); )*
                                __velo_cf_frag
                            }
                        };
                        // `Show` shows content when `when` is truthy. `Suspense`'s
                        // `loading` predicate is inverted: content is shown once the
                        // resource is DONE loading, fallback while still loading.
                        let predicate = match when_expr {
                            // Reactive predicate closure (`when={ move || bool }`):
                            // bind it once (its move-captured environment would
                            // otherwise be re-moved on every call), then invoke /
                            // invert it per effect run.
                            Some(v) if matches!(v, syn::Expr::Closure(_)) => {
                                let neg = if component_name == "Suspense" {
                                    quote! { ! }
                                } else {
                                    quote! {}
                                };
                                quote! { { let __velo_when = #v; move || #neg __velo_when() } }
                            }
                            // Any other value (signal, memo, plain bool): auto-unwrap
                            // via `signal_value!` inside a fresh tracked closure.
                            Some(v) => {
                                let when_val = reactive_value(v);
                                let neg = if component_name == "Suspense" {
                                    quote! { ! }
                                } else {
                                    quote! {}
                                };
                                quote! { move || #neg #when_val }
                            }
                            // No `when`/`loading` given: Show collapsed, Suspense shows
                            // content (loading is done).
                            None => {
                                if component_name == "Suspense" {
                                    quote! { move || !false }
                                } else {
                                    quote! { move || false }
                                }
                            }
                        };
                        tokens.extend(quote! {
                            veloasm::reactive_switch(
                                #predicate,
                                #content_block,
                                #fallback_val,
                            )
                        });
                        return;
                    }

                    // `Link` is a known builtin with an unusual props shape: its
                    // `label` / `active_class` fields are `Option<&str>` and its
                    // `children` is `Option<Vec<DomNode>>`. String-literal attrs
                    // (`label="Home"`, `active_class="is-active"`) are wrapped in
                    // `Some(..)` automatically so the long-standing `label=".."`
                    // API keeps working next to the children form
                    // (`<Link to="..">Text</Link>`); braced exprs pass through
                    // as-is so callers can also hand an `Option` value.
                    if component_name == "Link" {
                        let mut to_val: TokenStream2 = quote! {};
                        let mut label_val: TokenStream2 = quote! { None };
                        let mut class_val: TokenStream2 = quote! { None };
                        let mut prefetch_val: TokenStream2 = quote! { false };
                        for attr in attributes {
                            let v = &attr.value;
                            let v_optional: TokenStream2 = match v {
                                Expr::Lit(_) => quote! { Some(#v) },
                                _ => quote! { #v },
                            };
                            if attr.key == "to" {
                                // `LinkProps.to` is a `String`, so accept both a
                                // literal/const `&str` and the typed `paths::*`
                                // builders (String) via one `.into()`.
                                // 5.P9: when `to` is a string literal and the crate
                                // has an `src/app/` layout, validate it at compile
                                // time (`typedRoutes` parity) so `to="/typo"` fails
                                // to build instead of silently 404-ing.
                                if let Expr::Lit(expr_lit) = v {
                                    if let syn::Lit::Str(s) = &expr_lit.lit {
                                        if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
                                            let base_dir =
                                                Path::new(&manifest).join("src").join("app");
                                            if base_dir.is_dir() {
                                                if let Err(msg) =
                                                    validate_route_literal(&s.value(), &base_dir)
                                                {
                                                    let msg_lit = syn::LitStr::new(
                                                        &format!("invalid `to` route: {msg}"),
                                                        s.span(),
                                                    );
                                                    // Emit a single, on-target compile error and
                                                    // still yield a valid `String` so the rest of
                                                    // the element doesn't cascade.
                                                    to_val = quote! {
                                                        { compile_error!(#msg_lit); #v.into() }
                                                    };
                                                    continue;
                                                }
                                            }
                                        }
                                    }
                                }
                                to_val = quote! { #v.into() };
                            } else if attr.key == "label" {
                                label_val = v_optional;
                            } else if attr.key == "active_class" {
                                class_val = v_optional;
                            } else if attr.key == "prefetch" {
                                // `<Link prefetch />` → `true` (bare bool attr);
                                // `<Link prefetch={expr} />` → the bool expr.
                                prefetch_val = quote! { #v };
                            }
                        }
                        let children_val: TokenStream2 = if children.is_empty() {
                            quote! { None }
                        } else {
                            quote! { Some(vec![#(#children),*]) }
                        };
                        tokens.extend(quote! {
                            veloasm::Link(veloasm::LinkProps {
                                to: #to_val,
                                label: #label_val,
                                active_class: #class_val,
                                prefetch: #prefetch_val,
                                children: #children_val,
                            })
                        });
                        return;
                    }

                    // `Head` is a known builtin whose `title` / `meta` fields are
                    // `Option<..>`. Mirror `Link`'s handling so a string-literal
                    // `title="My App"` is wrapped in `Some(..)` automatically,
                    // while a braced `meta={ vec![("description", "...")] }`
                    // passes through as-is.
                    if component_name == "Head" {
                        let mut title_val: TokenStream2 = quote! { None };
                        let mut meta_val: TokenStream2 = quote! { None };
                        let mut has_title = false;
                        let mut has_meta = false;
                        for attr in attributes {
                            let v = &attr.value;
                            if attr.key == "title" {
                                has_title = true;
                                // `HeadProps.title` is `Option<String>`: string
                                // literals (`&str`) and braced `String`s alike pass
                                // through `.into()`, wrapped in `Some(..)`.
                                title_val = quote! { Some((#v).into()) };
                            } else if attr.key == "meta" {
                                has_meta = true;
                                meta_val = quote! { #v };
                            }
                        }
                        // `HeadProps.meta` is `Option<Vec<(String, String)>>`, so a
                        // braced `meta={ vec![..] }` (or array) needs `Some(..)`.
                        let title_final: TokenStream2 = if has_title { title_val } else { quote! { None } };
                        let meta_final: TokenStream2 = if has_meta {
                            quote! { Some(#meta_val) }
                        } else {
                            quote! { None }
                        };
                        tokens.extend(quote! {
                            veloasm::Head(veloasm::HeadProps {
                                title: #title_final,
                                meta: #meta_final,
                            })
                        });
                        return;
                    }


                    // All other components use a generated `<Name>Props` struct.
                    // `#[component]` synthesizes `NameProps` with one field per
                    // parameter, so attributes are matched BY NAME and may appear
                    // in any order. Children (either nested nodes or a `children=`
                    // attribute) map to the `children` field to support
                    // `<Panel>{..}</Panel>` composition.
                    let props_ident =
                        syn::Ident::new(&format!("{}Props", component_name), proc_macro2::Span::call_site());

                    let mut children_field: Option<TokenStream2> = None;
                    let mut struct_fields: Vec<TokenStream2> = Vec::new();
                    for attr in attributes {
                        if attr.key == "children" {
                            children_field = Some(quote! { #attr.value });
                        } else {
                            let key = syn::Ident::new(&attr.key, proc_macro2::Span::call_site());
                            let val = &attr.value;
                            struct_fields.push(quote! { #key: #val });
                        }
                    }
                    let children_present = !children.is_empty();
                    if children_present || children_field.is_some() {
                        let children_expr = match children_field {
                            Some(c) => c,
                            None => quote! { vec![ #(#children),* ] },
                        };
                        struct_fields.push(quote! { children: #children_expr });
                    }

                    tokens.extend(quote! {
                        #component_ident(#props_ident {
                            #(#struct_fields,)*
                        })
                    });
                } else {
                    let mut setup_statements = Vec::new();

                    setup_statements.push(quote! {
                        let parent_node = veloasm::DomNode::element(#tag_name);
                    });

                    // Collect every class-related attribute so we can coordinate them
                    // through a single `reactive_classes` call. `class:name={ bool }`
                    // bindings and a plain `class="..."` both write the same `class`
                    // attribute; coordinating them in one registry is what stops them
                    // from clobbering each other.
                    let mut base_classes: Vec<&syn::Expr> = Vec::new();
                    let mut toggle_classes: Vec<(String, &syn::Expr)> = Vec::new();

                    for attr in attributes {
                        let key = &attr.key;
                        let val = &attr.value;

                        if key == "on:submit" {
                            // Form submit sugar: calls `prevent_default()` (so the
                            // WASM app handles the action instead of the browser
                            // reloading/navigating) and forwards the event on.
                            // Bound to a local first: calling a closure literal
                            // directly (`#val(e)`) misparses at the `{...}(e)`.
                            setup_statements.push(quote! {
                                let submit_handler = #val;
                                parent_node.on("submit", move |e: web_sys::Event| {
                                    e.prevent_default();
                                    submit_handler(e)
                                });
                            });
                        } else if key.starts_with("on:") {
                            let event_type = key.strip_prefix("on:").unwrap();
                            setup_statements.push(quote! {
                                parent_node.on(#event_type, #val);
                            });
                        } else if key.starts_with("class:") {
                            // Reactive class toggle: class:active={ is_on }
                            let class_name = key.strip_prefix("class:").unwrap().to_string();
                            toggle_classes.push((class_name, val));
                        } else if key == "class" {
                            // Plain class="..." attribute: gathered with any toggles.
                            base_classes.push(val);
                        } else if key.starts_with("style:") {
                            // Reactive inline style: style:color={ color }
                            let prop = key.strip_prefix("style:").unwrap().to_string();
                            let rv = reactive_value(val);
                            setup_statements.push(quote! {
                                parent_node.reactive_style(#prop, move || #rv);
                            });
                        } else if key.starts_with("bind:value") {
                            // Two-way binding for text inputs, textareas, selects.
                            // 1. Hook the "input" event: every keystroke reads the element
                            //    and sets the signal so the signal tracks the DOM.
                            // 2. Reactive forwarding: when the signal is set externally
                            //    (e.g. clearing the field after "Add"), push it back to
                            //    the element's value attribute.
                            //
                            // Evaluate (#val) once into a temporary, then clone that
                            // temporary into each generated closure so there's no shared
                            // variable that both closures try to move.
                            //
                            // Closures capture the raw Node (a JS handle), never a
                            // `DomNode` clone: the effect is retained BY the node's
                            // resource, so capturing the node itself would form an
                            // ownership cycle (res -> effect -> closure -> res).
                            let event_name_str: syn::LitStr = syn::parse_quote! { "input" };
                            let field_name: syn::LitStr = syn::parse_quote! { "value" };
                            setup_statements.push(quote! {
                                let bind_tmp = (#val).clone();
                                let bind_sig_1 = bind_tmp.clone();
                                let bind_sig_2 = bind_tmp.clone();
                                // Forward signal -> DOM: set live property AND reactive attribute.
                                {
                                    let bind_node = parent_node.raw_node.clone();
                                    let bind_sig_1 = bind_sig_1.clone();
                                    parent_node.reactive_attribute(#field_name, move || {
                                        use wasm_bindgen::JsCast;
                                        let v = veloasm::signal_value!(bind_sig_1);
                                        let s = format!("{}", v);
                                        // Also set live DOM property for correct IME/reset behavior
                                        if let Ok(el) = bind_node.clone().dyn_into::<web_sys::HtmlInputElement>() {
                                            let _ = el.set_value(&s);
                                        }
                                        s
                                    });
                                }
                                // Forward DOM -> signal (on input, read element and set signal).
                                {
                                    let bind_node = parent_node.raw_node.clone();
                                    parent_node.on(#event_name_str, move |e: web_sys::Event| {
                                        use wasm_bindgen::JsCast;
                                        let target = e.target().expect("bind:value event has no target");
                                        let el = target.dyn_into::<web_sys::HtmlInputElement>().expect("bind:value requires an input/textarea/select element");
                                        bind_sig_2.set(el.value());
                                    });
                                }
                            });
                        } else if key.starts_with("bind:checked") {
                            // Two-way binding for checkboxes / radio buttons.
                            // 1. Hook the "change" event: toggled checkbox writes its
                            //    checked state back into the signal.
                            // 2. Reactive forwarding: external signal changes push
                            //    their state into the element's checked attribute.
                            //
                            // Evaluate (#val) once into a temporary, then clone that
                            // temporary into each generated closure so there's no shared
                            // variable that both closures try to move.
                            let event_name_str: syn::LitStr = syn::parse_quote! { "change" };
                            setup_statements.push(quote! {
                                let bind_tmp = (#val).clone();
                                let bind_sig_1 = bind_tmp.clone();
                                let bind_sig_2 = bind_tmp.clone();
                                // Forward signal -> DOM: set live property AND reactive attribute.
                                {
                                    let bind_node = parent_node.raw_node.clone();
                                    let bind_sig_1 = bind_sig_1.clone();
                                    parent_node.reactive_attribute("checked", move || {
                                        use wasm_bindgen::JsCast;
                                        let checked = veloasm::signal_value!(bind_sig_1);
                                        // `reactive_attribute` requires `FnMut() -> String`,
                                        // so materialise an owned String (not `&str`).
                                        let s: String = if checked { "checked".into() } else { "".into() };
                                        // Also set live DOM property for correct behavior
                                        if let Ok(el) = bind_node.clone().dyn_into::<web_sys::HtmlInputElement>() {
                                            let _ = el.set_checked(checked);
                                        }
                                        s
                                    });
                                }
                                // Forward DOM -> signal (on change, read element and set signal).
                                {
                                    let bind_node = parent_node.raw_node.clone();
                                    parent_node.on(#event_name_str, move |e: web_sys::Event| {
                                        use wasm_bindgen::JsCast;
                                        let target = e.target().expect("bind:checked event has no target");
                                        let el = target.dyn_into::<web_sys::HtmlInputElement>().expect("bind:checked requires a checkbox/radio input");
                                        bind_sig_2.set(el.checked());
                                    });
                                }
                            });
                        } else if key == "disabled"
                            || key == "checked"
                            || key == "selected"
                            || key == "readonly"
                        {
                            // Literal flags (`disabled=true`) are a one-shot DOM
                            // write — no effect, no closure. Anything reactive
                            // keeps an effect, retained by the node (and adopted
                            // by its enclosing scope) instead of being dropped.
                            if let syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Bool(b),
                                ..
                            }) = val
                            {
                                let lit = b.value;
                                let setter = if lit {
                                    quote! { parent_node.set_attr(#key, ""); }
                                } else {
                                    quote! { parent_node.remove_attr(#key); }
                                };
                                setup_statements.push(setter);
                            } else {
                                setup_statements.push(quote! {
                                    let p_node = parent_node.raw_node.clone();
                                    parent_node.retain_effect(veloasm::create_effect({
                                        let val_sig = move || #val;
                                        move || {
                                            use wasm_bindgen::JsCast;
                                            if let Ok(el) = p_node.clone().dyn_into::<web_sys::Element>() {
                                                if val_sig() {
                                                    let _ = el.set_attribute(#key, "");
                                                } else {
                                                    let _ = el.remove_attribute(#key);
                                                }
                                            }
                                        }
                                    }));
                                });
                            }
                        } else if let syn::Expr::Lit(syn::ExprLit { lit, .. }) = val {
                            // Literal string/int/bool attributes (`placeholder="Search"`,
                            // `tabindex="0"`, `hidden=false`): a direct DOM write —
                            // creating an effect per literal would waste memory on
                            // every static tag. Floats (and anything exotic) keep the
                            // reactive path to avoid suffix/formatting quirks.
                            let lit_str: Option<String> = match lit {
                                syn::Lit::Str(s) => Some(s.value()),
                                syn::Lit::Int(i) => Some(i.base10_digits().to_string()),
                                syn::Lit::Bool(b) => Some(b.value.to_string()),
                                _ => None,
                            };
                            if let Some(lit_str) = lit_str {
                                setup_statements.push(quote! {
                                    parent_node.set_attr(#key, #lit_str);
                                });
                            } else {
                                let rv = reactive_value(val);
                                setup_statements.push(quote! {
                                    parent_node.reactive_attribute(#key, move || format!("{}", #rv));
                                });
                            }
                        } else {
                            let rv = reactive_value(val);
                            setup_statements.push(quote! {
                                parent_node.reactive_attribute(#key, move || format!("{}", #rv));
                            });
                        }
                    }

                    // Coordinate class attributes gathered above. When the element
                    // carries any `class:` toggle, fold the whole class set into a
                    // single `reactive_classes` call (shared registry) so the base
                    // `class="..."` and the toggled classes stop overwriting one
                    // another. Static string-literal bases join the base; reactive
                    // `class={...}` expressions still use reactive_attribute.
                    if !toggle_classes.is_empty() {
                        let mut static_base: Vec<String> = Vec::new();
                        let mut reactive_base: Vec<&syn::Expr> = Vec::new();
                        for base in &base_classes {
                            if let syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Str(s),
                                ..
                            }) = base
                            {
                                static_base.push(s.value());
                            } else {
                                reactive_base.push(base);
                            }
                        }
                        let base_static = static_base.join(" ");

                        let toggle_calls: Vec<proc_macro2::TokenStream> = toggle_classes
                            .iter()
                            .map(|(name, val)| {
                                let name_lit = syn::LitStr::new(name, proc_macro2::Span::call_site());
                                let rv = reactive_value(val);
                                quote! {
                                    (#name_lit, Box::new(move || #rv) as Box<dyn FnMut() -> bool + 'static>),
                                }
                            })
                            .collect();

                        if reactive_base.is_empty() {
                            setup_statements.push(quote! {
                                parent_node.reactive_classes(#base_static, vec![ #(#toggle_calls)* ]);
                            });
                        } else {
                            // A reactive `class={...}` base and `class:` toggles
                            // must share ONE writer on the `class` attribute.
                            // Emitting `reactive_classes` + a separate
                            // `reactive_attribute("class", ...)` lets whichever
                            // effect re-runs last wipe the other's classes
                            // (e.g. base "log-row info" collapsing to "").
                            // Fold everything into a single reactive_attribute
                            // effect that reads the base and every toggle, so
                            // one effect owns the attribute end-to-end.
                            let reactive_vals: Vec<proc_macro2::TokenStream> = reactive_base
                                .iter()
                                .map(|rb| reactive_value(rb))
                                .collect();
                            let toggle_checks: Vec<proc_macro2::TokenStream> = toggle_classes
                                .iter()
                                .map(|(name, val)| {
                                    let name_lit = syn::LitStr::new(name, proc_macro2::Span::call_site());
                                    let rv = reactive_value(val);
                                    quote! { if #rv { __parts.push(#name_lit.to_string()); } }
                                })
                                .collect();
                            setup_statements.push(quote! {
                                parent_node.reactive_attribute("class", move || {
                                    let mut __parts: Vec<String> = Vec::new();
                                    if !#base_static.is_empty() { __parts.push(#base_static.to_string()); }
                                    #(
                                        {
                                            let __b: String = format!("{}", #reactive_vals);
                                            for __c in __b.split_whitespace() {
                                                __parts.push(__c.to_string());
                                            }
                                        }
                                    )*
                                    #(#toggle_checks)*
                                    __parts.join(" ")
                                });
                            });
                        }
                    } else {
                        for base in base_classes {
                            if let syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Str(s),
                                ..
                            }) = base
                            {
                                // Static `class="..."` with no toggles: one direct
                                // write instead of a lifetime-long effect.
                                let joined = s.value();
                                setup_statements.push(quote! {
                                    parent_node.set_attr("class", #joined);
                                });
                            } else {
                                let rv = reactive_value(base);
                                setup_statements.push(quote! {
                                    parent_node.reactive_attribute("class", move || format!("{}", #rv));
                                });
                            }
                        }
                    }

                    for child in children {
                        setup_statements.push(quote! {
                            parent_node.append(&#child);
                        });
                    }

                    tokens.extend(quote! {
                        {
                            #(#setup_statements)*
                            parent_node
                        }
                    });
                }
            }
        }
    }
}

#[proc_macro]
pub fn view(input: TokenStream) -> TokenStream {
    let parsed_root = parse_macro_input!(input as VNode);
    TokenStream::from(parsed_root.to_token_stream())
}

/// `#[component]` turns a plain function into a veloasm component and converts it
/// to the named-props form.
///
/// veloasm components are callable with **named props** in any order. The macro
/// rewrites the function so that:
///
/// 1. A `<Name>Props` struct is generated with one `pub` field per parameter.
/// 2. The function now takes that struct as a single `props` argument and
///    destructures it into the original parameter names.
/// 3. The return type is rewritten to `veloasm::DomNode` so the body can end with
///    a `view! { ... }` tail expression (no explicit `-> DomNode` needed).
///
/// The `view!` macro matches JSX attributes to the struct fields by name, so
/// `<UserCard role="Admin" name="Ada" />` resolves correctly regardless of
/// order. A special `children` field receives child nodes, enabling
/// `<Panel>{..}</Panel>` composition.
///
/// ```ignore
/// #[veloasm::component]
/// fn UserCard(name: String, active: bool) {
///     view! {
///         <div class:active={ active }>
///             <p>"Hello, " { name }</p>
///         </div>
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut func = match syn::parse::<syn::ItemFn>(item) {
        Ok(f) => f,
        Err(e) => return TokenStream::from(e.to_compile_error()),
    };

    if !attr.is_empty() {
        return TokenStream::from(
            syn::Error::new_spanned(
                &func.sig.ident,
                "veloasm #[component] no longer takes a props-type argument: the <Name>Props struct is generated automatically from the parameter list",
            )
            .to_compile_error(),
        );
    }

    let fn_name = &func.sig.ident;
    let props_ident = syn::Ident::new(&format!("{}Props", fn_name), proc_macro2::Span::call_site());

    // Collect `(field_ident, field_type)` from each typed parameter.
    let mut fields: Vec<(syn::Ident, syn::Type)> = Vec::new();
    for input in &mut func.sig.inputs {
        let arg = match input {
            syn::FnArg::Typed(pt) => pt,
            syn::FnArg::Receiver(_) => {
                return TokenStream::from(
                    syn::Error::new_spanned(input, "veloasm #[component] cannot take a `self` receiver")
                        .to_compile_error(),
                );
            }
        };
        let field_ident = match &*arg.pat {
            syn::Pat::Ident(pi) => pi.ident.clone(),
            _ => {
                return TokenStream::from(
                    syn::Error::new_spanned(
                        arg,
                        "veloasm #[component] parameters must be plain named identifiers",
                    )
                    .to_compile_error(),
                );
            }
        };
        fields.push((field_ident, (*arg.ty).clone()));
    }

    let field_defs = fields.iter().map(|(id, ty)| quote! { pub #id: #ty });
    let props_struct = quote! {
        #[allow(non_snake_case)]
        pub struct #props_ident {
            #(#field_defs,)*
        }
    };

    // Destructure props back into the original parameter names inside the body.
    let field_names: Vec<_> = fields.iter().map(|(id, _)| id).collect();
    let destructure: syn::Stmt = syn::parse_quote! {
        let #props_ident { #(#field_names,)* } = props;
    };

    // Swap the parameter list for a single `props: <Name>Props` argument.
    func.sig.inputs.clear();
    func.sig
        .inputs
        .push(syn::parse_quote! { props: #props_ident });

    // Wrap the whole body in `veloasm::scoped_node`: every effect created while
    // the component builds (`effect!`, helper bindings, nested components'
    // leftovers) is adopted by the scope and attached to the returned node,
    // so tearing the node down disposes them instead of leaking for the app
    // lifetime. Early `return`s inside the body return from the closure, so
    // the scope always closes.
    let existing_stmts = std::mem::take(&mut func.block.stmts);
    let wrapped: syn::Expr = syn::parse_quote! {
        veloasm::scoped_node(move || {
            #destructure
            #(#existing_stmts)*
        })
    };
    func.block.stmts.push(syn::Stmt::Expr(wrapped, None));

    // Force the return type to DomNode.
    func.sig.output = syn::ReturnType::Type(
        Default::default(),
        Box::new(syn::parse_quote!(veloasm::DomNode)),
    );

    // Components are conventionally PascalCase (e.g. `UserCard`), so suppress
    // the `non_snake_case` lint on the function name. Without this every
    // `#[component] fn Foo(...)` in user apps would warn.
    func.attrs.push(syn::parse_quote! {
        #[allow(non_snake_case)]
    });

    TokenStream::from(quote! {
        #props_struct
        #func
    })
}

/// Declarative macro for defining route tables.
#[proc_macro]
pub fn routes(input: TokenStream) -> TokenStream {
    let routes = parse_macro_input!(input as RouteList);

    let route_entries = routes
        .0
        .iter()
        .map(|(path, component)| {
            let path_lit = path;
            let comp = component;
            quote! {
                veloasm::Route {
                    path: #path_lit,
                    component: #comp,
                }
            }
        });

    let expanded = quote! {
        vec![#(#route_entries),*]
    };

    TokenStream::from(expanded)
}

/// Parse the full route list.
struct RouteList(Vec<(LitStr, Ident)>);

impl syn::parse::Parse for RouteList {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut entries = Vec::new();
        while !input.is_empty() {
            let path: LitStr = input.parse()?;
            input.parse::<syn::Token![=>]>()?;
            let comp: Ident = input.parse()?;
            if !input.is_empty() {
                input.parse::<syn::Token![,]>()?;
            }
            entries.push((path, comp));
        }
        Ok(RouteList(entries))
    }
}

#[proc_macro_attribute]
pub fn route(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Invoke as `#[route("/path")]`. For a `#[attr(...)]` proc-macro attribute,
    // rustc hands the macro only the tokens *inside* the parens, so `attr` is
    // just the path literal (no parentheses, no `=`).
    struct RouteAttr(syn::LitStr);
    impl syn::parse::Parse for RouteAttr {
        fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
            Ok(RouteAttr(input.parse()?))
        }
    }
    let RouteAttr(path_lit) = parse_macro_input!(attr as RouteAttr);
    let mut func = match syn::parse::<syn::ItemFn>(item) {
        Ok(f) => f,
        Err(e) => return TokenStream::from(e.to_compile_error()),
    };
    if !func.sig.inputs.is_empty() {
        return TokenStream::from(syn::Error::new_spanned(
            &func.sig.inputs,
            "veloasm: #[route] components take no arguments — read params via AppRouter::use_param",
        ).to_compile_error());
    }
    func.sig.output = syn::ReturnType::Type(Default::default(), Box::new(syn::parse_quote!(veloasm::DomNode)));
    let fn_ident = func.sig.ident.clone();
    TokenStream::from(quote! {
        #func
        veloasm::inventory::submit! {
            veloasm::RouteRegistration { path: #path_lit, component: #fn_ident }
        }
    })
}

// =============================================================================
// §4 — File-based `app/` routing: `veloasm::app!` + `#[page]` / `#[layout]`
// =============================================================================

/// Marker for an application **page** function.
///
/// By convention the function is named `page` and lives at
/// `src/app/**/page.rs`, e.g. `src/app/blog/[slug]/page.rs` ->
/// `/blog/:slug`. Returns `veloasm::DomNode`; read route params with
/// `AppRouter::use_param::<T>("name")`.
#[proc_macro_attribute]
pub fn page(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Marker for a **layout** function.
///
/// By convention named `layout` at `src/app/**/layout.rs`. Takes the matched
/// child subtree and wraps it: `fn layout(child: DomNode) -> DomNode`. Layouts
/// from the nearest segment up to the root compose around every route below
/// them.
#[proc_macro_attribute]
pub fn layout(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Marker for the global **not-found** page at `src/app/not-found.rs`.
/// By convention: `fn not_found() -> DomNode`. Registers the `/**` fallback.
#[proc_macro_attribute]
pub fn not_found(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Marker for a segment **error** boundary at `src/app/**/error.rs`.
/// By convention: `fn error() -> DomNode`. `app!` wraps every page below the
/// nearest `error.rs` in an error boundary (see `veloasm::error_boundary`).
#[proc_macro_attribute]
pub fn error(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Marker for a segment **loading** placeholder at `src/app/**/loading.rs`.
/// By convention: `fn loading() -> DomNode`. `app!` shows it as a Suspense
/// fallback while the route is mounting (`veloasm::route_loading()`).
#[proc_macro_attribute]
pub fn loading(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[derive(PartialEq, Clone, Copy)]
enum AppFileKind {
    Page,
    Layout,
    NotFound,
    Other,
}

struct AppFile {
    rel: String,
    kind: AppFileKind,
}

fn collect_app_files(base: &Path) -> Vec<AppFile> {
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                stack.push(path);
            } else if path.extension().map(|x| x == "rs").unwrap_or(false) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let rel = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let kind = match name.as_str() {
                    "page.rs" => AppFileKind::Page,
                    "layout.rs" => AppFileKind::Layout,
                    "not-found.rs" => AppFileKind::NotFound,
                    _ => AppFileKind::Other,
                };
                out.push(AppFile { rel, kind });
            }
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

/// Deterministic module name for a relative `src/app/` file path.
fn module_ident(rel: &str) -> syn::Ident {
    let stem = rel.strip_suffix(".rs").unwrap_or(rel);
    let mut name = String::new();
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() {
            name.push(ch.to_ascii_lowercase());
        } else {
            name.push('_');
        }
    }
    if name.is_empty() || name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        name.insert(0, 'p');
    }
    syn::Ident::new(&name, proc_macro2::Span::call_site())
}

fn sanitize_ident(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    if out.is_empty() || out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, 'p');
    }
    out
}

#[derive(Default)]
struct RouteData {
    path: String,
    /// Raw param keys, in route order (`[x]` -> `x`).
    params: Vec<String>,
    /// `format!(..)` template with `{}` slots in param order.
    fmt: String,
    /// Static segment idents (for const names), in order.
    static_segs: Vec<String>,
}

fn route_data(rel: &str) -> RouteData {
    let stem = rel.strip_suffix(".rs").unwrap_or(rel);
    let mut segs: Vec<&str> = stem.split('/').collect();
    segs.pop(); // drop the `page` filename
    let mut data = RouteData { path: "/".to_string(), fmt: "/".to_string(), ..Default::default() };
    for seg in segs {
        let mut catch_all = false;
        if let Some(inner) = seg.strip_prefix("[...").and_then(|s| s.strip_suffix(']')) {
            data.params.push(inner.to_string());
            data.fmt.push_str("{}");
            catch_all = true;
        } else if let Some(inner) = seg.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            data.params.push(inner.to_string());
            data.path.push(':');
            data.path.push_str(inner);
            data.fmt.push_str("{}");
        } else {
            data.static_segs.push(sanitize_ident(seg));
            data.path.push_str(seg);
            data.fmt.push_str(seg);
        }
        data.path.push('/');
        data.fmt.push('/');
        if catch_all {
            data.path.push_str("**");
            data.fmt.pop();
            break;
        }
    }
    if data.path.len() > 1 && !data.path.ends_with("**") {
        data.path.pop();
    }
    if data.fmt.len() > 1 && !data.fmt.ends_with("**") {
        data.fmt.pop();
    }
    data
}

/// A single path segment in a route pattern (5.P9 typed navigation).
#[derive(Clone, PartialEq)]
enum RouteSeg {
    /// Literal segment, e.g. `blog`.
    Lit(String),
    /// Dynamic param slot (`:x` / `[x]`) — matches any one segment.
    Param,
    /// Catch-all (`**` / `[...x]`) — matches the remaining path.
    CatchAll,
}

/// Turns a route `path` string (from [`route_data`]) into a segment list that
/// [`route_path!`] and `<Link to>` validation can match a concrete URL against.
fn route_pattern(path: &str) -> Vec<RouteSeg> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(|seg| {
            if seg == "**" {
                RouteSeg::CatchAll
            } else if seg.starts_with(':') {
                RouteSeg::Param
            } else {
                RouteSeg::Lit(seg.to_string())
            }
        })
        .collect()
}

/// Does concrete URL `input` match this route pattern? A `Param` consumes
/// exactly one input segment; a trailing `CatchAll` consumes the rest; a `Lit`
/// must equal its input segment. Leftover pattern (non-catch-all) or input
/// segments fail the match, so genuinely-missing routes are rejected while
/// params stay loose.
fn pattern_matches(pat: &[RouteSeg], input: &[&str]) -> bool {
    let mut i = 0; // input index
    for (pos, seg) in pat.iter().enumerate() {
        match seg {
            RouteSeg::CatchAll => {
                // Must consume at least one segment and be last.
                return pos + 1 == pat.len() && i < input.len();
            }
            RouteSeg::Param => {
                if i >= input.len() {
                    return false;
                }
                i += 1;
            }
            RouteSeg::Lit(lit) => {
                if i >= input.len() || input[i] != lit {
                    return false;
                }
                i += 1;
            }
        }
    }
    i == input.len()
}

/// Compile-time route registry: scans `src/app/` and returns the pattern for
/// every *declared* page route. A declared `[...rest]` catch-all is included so
/// it validates explicitly; the implicit not-found fallback is deliberately
/// excluded here — it would otherwise accept every path and defeat the
/// `typedRoutes`-style check.
fn discover_route_patterns(base: &Path) -> Vec<(String, Vec<RouteSeg>)> {
    let mut out: Vec<(String, Vec<RouteSeg>)> = Vec::new();
    if !base.is_dir() {
        return out;
    }
    let files = collect_app_files(base);
    for f in files.iter().filter(|f| f.kind == AppFileKind::Page) {
        let data = route_data(&f.rel);
        out.push((data.path.clone(), route_pattern(&data.path)));
    }
    out
}

/// Validates a concrete URL literal against the discovered routes, returning a
/// human-friendly error message (listing available routes) on failure.
fn validate_route_literal(input: &str, base: &Path) -> core::result::Result<(), String> {
    let patterns = discover_route_patterns(base);
    let input_segs: Vec<&str> = input.split('/').filter(|s| !s.is_empty()).collect();
    let root_matches = input == "/" || (input_segs.is_empty() && input.is_empty());
    for (path, pat) in &patterns {
        if root_matches && path == "/" {
            return Ok(());
        }
        if pattern_matches(pat, &input_segs) {
            return Ok(());
        }
    }
    let mut listing = patterns
        .iter()
        .map(|(p, _)| format!("      - {p}"))
        .collect::<Vec<_>>()
        .join("\n");
    if listing.is_empty() {
        listing = "      (no `page.rs` files found under src/app/)".to_string();
    }
    Err(format!(
        "no route in `src/app/` matches \"{input}\"\navailable routes:\n{listing}"
    ))
}


/// Layout chain (nearest segment layout -> ... -> root layout) for a page rel.
fn layout_chain(rel: &str, files: &[AppFile]) -> Vec<String> {
    let dir = match rel.rsplit_once('/') {
        Some((d, _)) => d,
        None => "",
    };
    let mut dirs = Vec::new();
    if !dir.is_empty() {
        let mut cur = String::new();
        for part in dir.split('/') {
            cur.push_str(part);
            dirs.push(cur.clone());
            cur.push('/');
        }
    }
    let mut out = Vec::new();
    for d in dirs.iter().rev() {
        let layout_rel = if d.is_empty() { "layout.rs".into() } else { format!("{d}/layout.rs") };
        if files.iter().any(|f| f.rel == layout_rel && f.kind == AppFileKind::Layout) {
            out.push(layout_rel);
        }
    }
    if files.iter().any(|f| f.rel == "layout.rs" && f.kind == AppFileKind::Layout) {
        out.push("layout.rs".into());
    }
    out
}

/// Emits a typed `paths` entry for a page: `const` for static paths, a
/// path-building `fn` for routes with params.
fn path_helper(data: &RouteData) -> TokenStream2 {
    if data.params.is_empty() {
        let mut name = String::from("INDEX");
        if !data.static_segs.is_empty() {
            name = data
                .static_segs
                .iter()
                .map(|s| s.to_ascii_uppercase())
                .collect::<Vec<_>>()
                .join("_");
        }
        let const_name = syn::Ident::new(&name, proc_macro2::Span::call_site());
        let path_lit = syn::LitStr::new(&data.path, proc_macro2::Span::call_site());
        quote! { pub const #const_name: &str = #path_lit; }
    } else {
        let mut name = String::new();
        for seg in &data.static_segs {
            name.push_str(seg);
            name.push('_');
        }
        for p in &data.params {
            name.push_str(&sanitize_ident(p));
            name.push('_');
        }
        name.pop();
        let fn_name = syn::Ident::new(&name, proc_macro2::Span::call_site());
        let args: Vec<syn::Ident> = data
            .params
            .iter()
            .map(|p| syn::Ident::new(&sanitize_ident(p), proc_macro2::Span::call_site()))
            .collect();
        let fmt_lit = syn::LitStr::new(&data.fmt, proc_macro2::Span::call_site());
        quote! { pub fn #fn_name(#(#args: &str),*) -> String { format!(#fmt_lit, #(#args),*) } }
    }
}

/// `veloasm::app!()` — Next.js-style file-based routing.
///
/// Reads the crate's `src/app/` directory at compile time and expands to a
/// `pub mod veloasm_app { .. }` containing:
///   - one nested `mod` per `.rs` file (via `include!`),
///   - a typed `paths` module of compile-checked route path helpers,
///   - `pub fn routes() -> Vec<veloasm::Route>` to hand to `<Router />`.
///
/// Conventions:
/// ```text
///   src/app/page.rs              -> "/"
///   src/app/layout.rs            -> root layout (wraps every route)
///   src/app/blog/page.rs         -> "/blog"
///   src/app/blog/[slug]/page.rs  -> "/blog/:slug"
///   src/app/blog/[...rest]/page.rs -> "/blog/**"   (catch-all)
///   src/app/not-found.rs         -> "**"           (global not-found)
///   src/app/blog/error.rs        -> nearest error boundary (wraps below it)
///   src/app/blog/loading.rs      -> nearest loading placeholder
/// ```
///
/// Page files expose `pub fn page() -> veloasm::DomNode`; layout files expose
/// `pub fn layout(child: veloasm::DomNode) -> veloasm::DomNode` (they wrap the
/// matched leaf — the chain is *registered* with the Router, which keeps the
/// shell mounted and swaps only the leaf across sibling navigation);
/// `not-found.rs`, `error.rs`, `loading.rs` expose zero-arg
/// `pub fn <name>() -> veloasm::DomNode`. Params are read with
/// `AppRouter::use_param::<T>("slug")`.
#[proc_macro]
pub fn app(_input: TokenStream) -> TokenStream {
    let manifest = match std::env::var("CARGO_MANIFEST_DIR") {
        Ok(m) => m,
        Err(_) => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                "veloasm::app!: CARGO_MANIFEST_DIR is unavailable",
            )
            .to_compile_error()
            .into();
        }
    };
    let src_app: PathBuf = Path::new(&manifest).join("src").join("app");
    if !src_app.is_dir() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "veloasm::app! requires an `src/app/` directory at the crate root (Next.js-style)",
        )
        .to_compile_error()
        .into();
    }

    let files = collect_app_files(&src_app);
    let pages: Vec<&AppFile> = files.iter().filter(|f| f.kind == AppFileKind::Page).collect();
    if pages.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "veloasm::app!: no `page.rs` files found under `src/app/`",
        )
        .to_compile_error()
        .into();
    }

    // One compiled module per `src/app/` file. The included files reference the
    // typed `paths` helpers (e.g. `paths::blog_slug("x")`); `paths` lives in
    // `veloasm_app`, one level above every page module, so import it via `super`.
    let mod_items = files.iter().map(|f| {
        let name = module_ident(&f.rel);
        let rel_lit = syn::LitStr::new(&f.rel, proc_macro2::Span::call_site());
        quote! {
            mod #name {
                use super::paths;
                include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/app/", #rel_lit));
            }
        }
    });

    let mut wrappers = Vec::new();
    let mut route_entries = Vec::new();
    let mut path_items = Vec::new();
    let mut layout_entries = Vec::new();

    for page in &pages {
        let data = route_data(&page.rel);
        let leaf = module_ident(&page.rel);
        let render_name = syn::Ident::new(
            &format!("{leaf}__render"),
            proc_macro2::Span::call_site(),
        );
        let path_lit = syn::LitStr::new(&data.path, proc_macro2::Span::call_site());

        // Layouts are not inlined into the leaf — they're handed to the Router
        // as a registered chain so the shell stays mounted across sibling
        // navigation and only the leaf swaps (M4 / 5.P4).
        let layouts = layout_chain(&page.rel, &files);
        let layout_fns: Vec<TokenStream2> = layouts
            .iter()
            .map(|l| {
                let lm = module_ident(l);
                quote! { #lm::layout as veloasm::LayoutFn }
            })
            .collect();
        layout_entries.push(quote! {
            (#path_lit, &[#(#layout_fns),*])
        });

        // Per-route error/loading boundaries (M5 / 5.P5): the nearest
        // `error.rs` / `loading.rs` from the page's segment chain wins, else a
        // built-in fallback / no placeholder.
        let error_fallback = match find_segment_file(&page.rel, &files, "error.rs") {
            Some(er) => {
                let em = module_ident(&er);
                quote! { #em::error() }
            }
            None => quote! { veloasm::default_error_fallback() },
        };
        let loading_fallback = match find_segment_file(&page.rel, &files, "loading.rs") {
            Some(lr) => {
                let lm = module_ident(&lr);
                quote! {
                    veloasm::reactive_switch(move || !veloasm::route_loading(), __content, #lm::loading())
                }
            }
            None => quote! { __content },
        };

        wrappers.push(quote! {
            fn #render_name() -> veloasm::DomNode {
                let __content =
                    veloasm::error_boundary(#error_fallback, Box::new(move || #leaf::page()));
                #loading_fallback
            }
        });

        route_entries.push(quote! {
            veloasm::Route { path: #path_lit, component: #render_name }
        });
        path_items.push(path_helper(&data));
    }

    for nf in files.iter().filter(|f| f.kind == AppFileKind::NotFound) {
        let leaf = module_ident(&nf.rel);
        let render_name =
            syn::Ident::new(&format!("{leaf}__render"), proc_macro2::Span::call_site());
        let layouts = layout_chain(&nf.rel, &files);
        let layout_fns: Vec<TokenStream2> = layouts
            .iter()
            .map(|l| {
                let lm = module_ident(l);
                quote! { #lm::layout as veloasm::LayoutFn }
            })
            .collect();
        layout_entries.push(quote! {
            ("/**", &[#(#layout_fns),*])
        });
        wrappers.push(quote! {
            fn #render_name() -> veloasm::DomNode {
                veloasm::error_boundary(
                    veloasm::default_error_fallback(),
                    Box::new(move || #leaf::not_found()),
                )
            }
        });
        route_entries.push(quote! {
            veloasm::Route { path: "/**", component: #render_name }
        });
    }

    let expanded = quote! {
        pub mod veloasm_app {
            #(#mod_items)*
            pub mod paths {
                #(#path_items)*
            }
            #(#wrappers)*
            pub fn routes() -> Vec<veloasm::Route> {
                veloasm::register_app_layouts(&[
                    #(#layout_entries),*
                ]);
                vec![
                    #(#route_entries),*
                ]
            }
        }
    };
    TokenStream::from(expanded)
}

/// Nearest `error.rs` / `loading.rs` in the page's segment directory chain:
/// innermost segment first, walking up, then the app root.
fn find_segment_file(rel: &str, files: &[AppFile], fname: &str) -> Option<String> {
    let stem = rel.strip_suffix(".rs").unwrap_or(rel);
    let mut segs: Vec<&str> = stem.split('/').collect();
    segs.pop(); // drop the `page` filename
    for i in (0..segs.len()).rev() {
        let dir = segs[..=i].join("/");
        let candidate = format!("{dir}/{fname}");
        if files.iter().any(|f| f.rel == candidate) {
            return Some(candidate);
        }
    }
    let candidate = fname.to_string();
    if files.iter().any(|f| f.rel == candidate) {
        return Some(candidate);
    }
    None
}

/// `route_path!("/some/route")` — a compile-time validated path literal
/// (`typedRoutes` parity, 5.P9).
///
/// Scans the crate's `src/app/` layout and fails to compile if the literal
/// doesn't match a known page route (catches `to=` typos before they reach
/// `<Link>` / `navigate_to`). Dynamic params match any one segment; `**`
/// matches the rest, so `route_path!("/blog/anything")` is valid alongside
/// `/blog/[slug]`. Expands to the literal `&'static str`, so it drops straight
/// into `<Link to={ route_path!("/posts") } />` or `navigate_to(route_path!(..))`.
///
/// ```ignore
/// <Link to={ route_path!("/about") } label="About" />   // ok
/// navigate_to(route_path!("/typo-route"));              // compile error: no such route
/// ```
#[proc_macro]
pub fn route_path(input: TokenStream) -> TokenStream {
    let str_lit = parse_macro_input!(input as syn::LitStr);
    let value = str_lit.value();

    let base_dir = match std::env::var("CARGO_MANIFEST_DIR") {
        Ok(m) => Path::new(&m).join("src").join("app"),
        Err(_) => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                "route_path!: CARGO_MANIFEST_DIR is unavailable",
            )
            .to_compile_error()
            .into();
        }
    };

    if let Err(msg) = validate_route_literal(&value, &base_dir) {
        return syn::Error::new(str_lit.span(), format!("route_path! failed: {msg}"))
            .to_compile_error()
            .into();
    }

    let lit = syn::LitStr::new(&value, proc_macro2::Span::call_site());
    quote::quote! { #lit }.into()
}



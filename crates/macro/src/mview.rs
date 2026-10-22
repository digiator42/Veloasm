//! `mview!` — Maud-like syntax for native `veloasm::DomNode`.
//!
//! Shortest syntax, maximum sugar, native veloasm signals.
//!
//! ```ignore
//! let node = mview! {
//!     div.task-row class:done={ task.done } {
//!         span.task-text { task.title }
//!         button @click={ toggle("done") } {
//!             @if task.done { "Mark open" } @else { "Done" }
//!         }
//!     }
//! }
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::ext::IdentExt;
use syn::parse::discouraged::Speculative;
use syn::parse::{Parse, ParseStream, Result};
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, Expr, Ident, LitStr, Pat, Token};

use crate::{try_parse_arrow, ArrowClosure};

/// A Maud-style node. Unlike `VNode` (JSX-like), this uses Rust/Maud syntax:
/// `tag.class#id attr=value { children }`
enum MViewNode {
    /// Static text: `"hello"`
    Text(LitStr),
    /// Reactive expression: `{ expr }` — auto-unwraps signals
    Reactive(Expr),
    /// Element: `div.class#id attr=val { children }`
    Element(MViewElement),
    /// Fragment: `{ child1 child2 }` or `<> ... </>`
    Fragment(Vec<MViewNode>),
    /// Control flow: `@if cond { then } @else { otherwise }`
    If { cond: Expr, then: Vec<MViewNode>, otherwise: Option<Vec<MViewNode>> },
    /// Loop: `@for pat in expr { body }`
    For { pat: Pat, expr: Expr, body: Vec<MViewNode> },
    /// Let binding: `@let name = expr;`
    Let { name: Ident, value: Expr },
    /// Component: `ComponentName { prop=value }`
    Component { name: Ident, props: Vec<MViewProp>, children: Vec<MViewNode> },
    /// Spread attributes: `{ ..expr }`
    Spread(Expr),
}

/// An element in mview syntax.
struct MViewElement {
    tag: String,
    classes: Vec<String>,
    id: Option<String>,
    attrs: Vec<MViewAttr>,
    events: Vec<MViewEvent>,
    children: Vec<MViewNode>,
}

/// An attribute: `attr=value` or `attr={expr}` or `attr=|signal|`
struct MViewAttr {
    name: String,
    value: Option<MViewAttrValue>,
}

/// Attribute value with sugar support.
enum MViewAttrValue {
    Static(LitStr),
    Reactive(Expr),           // `{expr}` or `expr`
    SignalRef(String),        // `|signal|` pipe sugar
    Bool(bool),               // bare attribute
}

/// Event handler: `@click={...}` or `on:click={...}`
struct MViewEvent {
    name: String,
    handler: Expr,
}

/// Component prop: `prop=value` or `prop={expr}` or `prop=|signal|`
struct MViewProp {
    name: String,
    value: MViewPropValue,
}

enum MViewPropValue {
    Static(LitStr),
    Reactive(Expr),
    SignalRef(String),
    Bool(bool),
}

/// Parse mview input: a sequence of nodes.
struct MViewInput(Vec<MViewNode>);

impl Parse for MViewInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut nodes = Vec::new();
        while !input.is_empty() {
            nodes.push(parse_mview_node(input)?);
        }
        Ok(MViewInput(nodes))
    }
}

/// Main entry: parse a single mview node.
fn parse_mview_node(input: ParseStream) -> Result<MViewNode> {
    // Text literal
    if input.peek(LitStr) {
        return Ok(MViewNode::Text(input.parse()?));
    }

    // Block: `{ ... }` — could be reactive expr, fragment, or element body
    if input.peek(syn::token::Brace) {
        let content;
        syn::braced!(content in input);

        // Try reactive expression first
        if let Ok(expr) = content.parse::<Expr>() {
            if content.is_empty() {
                return Ok(MViewNode::Reactive(expr));
            }
        }

        // Try arrow closure for reactive
        if let Some(arrow) = try_parse_arrow(&content) {
            if content.is_empty() {
                return Ok(MViewNode::Reactive(*arrow.body));
            }
        }

        // It's a fragment - parse children
        let child_nodes = content.parse::<MViewInput>()?.0;
        return Ok(MViewNode::Fragment(child_nodes));
    }

    // Control flow: `@if`, `@for`, `@let`
    if input.peek(Token![@]) {
        input.parse::<Token![@]>()?;
        let kw = input.call(Ident::parse_any)?.to_string();
        return match kw.as_str() {
            "if" => parse_if(input),
            "else" => Err(syn::Error::new(input.span(), "unexpected @else, use @if ... @else")),
            "for" => parse_for(input),
            "let" => parse_let(input),
            _ => Err(syn::Error::new(input.span(), format!("unknown @{} directive", kw))),
        };
    }

    // Component (PascalCase) or Element (lowercase)
    if input.peek(Ident::peek_any) {
        let fork = input.fork();
        let ident = fork.call(Ident::parse_any)?;
        let name = ident.to_string();

        if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            // Component
            return parse_component(input, ident);
        }

        // Element - check what follows
        // Allow @ for event handlers (@click), . for class, # for id, ident for attributes, = for attr=value, { for children
        if fork.peek(Token![.]) || fork.peek(Token![#]) || fork.peek(Token![@]) || fork.peek(Ident::peek_any) || fork.peek(Token![=]) || fork.peek(syn::token::Brace) {
            return parse_element(input);
        }

        // Bare identifier - could be reactive expression
        return Ok(MViewNode::Reactive(input.parse()?));
    }

    // Fragment shorthand: `<> ... </>`
    if input.peek(Token![<]) && input.peek2(Token![>]) {
        input.parse::<Token![<]>()?;
        input.parse::<Token![>]>()?;
        let mut children = Vec::new();
        while !(input.peek(Token![<]) && input.peek2(Token![/])) {
            children.push(parse_mview_node(input)?);
        }
        input.parse::<Token![<]>()?;
        input.parse::<Token![/]>()?;
        input.parse::<Token![>]>()?;
        return Ok(MViewNode::Fragment(children));
    }

    Err(input.error("expected element, text, @directive, or component"))
}

/// Parse element: `tag.class#id attr=val { children }`
fn parse_element(input: ParseStream) -> Result<MViewNode> {
    let tag_ident = input.call(Ident::parse_any)?;
    let tag = tag_ident.to_string().to_ascii_lowercase();

    // Parse .class and #id
    let mut classes = Vec::new();
    let mut id = None;
    while input.peek(Token![.]) || input.peek(Token![#]) {
        if input.peek(Token![.]) {
            input.parse::<Token![.]>()?;
            let class_ident = input.call(Ident::parse_any)?;
            classes.push(class_ident.to_string());
        } else {
            input.parse::<Token![#]>()?;
            let id_ident = input.call(Ident::parse_any)?;
            id = Some(id_ident.to_string());
        }
    }

    let mut attrs = Vec::new();
    let mut events = Vec::new();

    // Parse attributes and events
    while !input.peek(syn::token::Brace) && !input.peek(Token![>]) && !input.peek(Token![/]) {
        // Event shorthand: @click={...} or @click=... or @click ... (no =)
        if input.peek(Token![@]) {
            input.parse::<Token![@]>()?;
            let event_ident = input.call(Ident::parse_any)?;
            let event_name = event_ident.to_string();
            // Support @click={...}, @click=..., and @click {...} (optional =)
            if input.peek(Token![=]) {
                input.parse::<Token![=]>()?;
            }
            let handler = crate::parse_event_handler_value(input)?;
            events.push(MViewEvent { name: event_name, handler });
            continue;
        }

        // Regular attribute - could be class:active=|signal| or class:active={...}
        let key_ident = input.call(Ident::parse_any)?;
        let mut key = key_ident.to_string();

        // Handle `class:active` or `style:color` compound keys (colon-separated)
        if input.peek(Token![:]) {
            input.parse::<Token![:]>()?;
            let second = input.call(Ident::parse_any)?;
            key.push(':');
            key.push_str(&second.to_string());
        }

        // Handle hyphenated attributes (data-*, aria-*, etc.)
        while input.peek(Token![-]) {
            key.push('-');
            key.push_str(&crate::parse_dash_segment(input)?);
        }

        // Special: `on:click=...` or `on:click={...}` or `on:click ...`
        if key.starts_with("on:") {
            if input.peek(Token![=]) {
                input.parse::<Token![=]>()?;
            }
            let handler = crate::parse_event_handler_value(input)?;
            events.push(MViewEvent { name: key, handler });
            continue;
        }

        // Special: `class:active=...` or `style:color=...` (reactive toggles)
        if key.starts_with("class:") || key.starts_with("style:") {
            if input.peek(Token![=]) {
                input.parse::<Token![=]>()?;
            }
            let value = parse_attr_value(input)?;
            attrs.push(MViewAttr { name: key, value: Some(value) });
            continue;
        }

        // Regular attr: `attr="value"` or `attr={expr}` or bare
        let value = if input.peek(Token![=]) {
            input.parse::<Token![=]>()?;
            Some(parse_attr_value(input)?)
        } else {
            None
        };
        attrs.push(MViewAttr { name: key, value });
    }

    // Children: either `{ ... }` or `> ... </tag>`
    let children = if input.peek(syn::token::Brace) {
        let content;
        syn::braced!(content in input);
        content.parse::<MViewInput>()?.0
    } else if input.peek(Token![>]) {
        input.parse::<Token![>]>()?;
        let mut children = Vec::new();
        while !(input.peek(Token![<]) && input.peek2(Token![/])) {
            children.push(parse_mview_node(input)?);
        }
        input.parse::<Token![<]>()?;
        input.parse::<Token![/]>()?;
        let end_tag = input.call(Ident::parse_any)?;
        if end_tag.to_string().to_ascii_lowercase() != tag {
            return Err(syn::Error::new(end_tag.span(), format!("mismatched closing tag: expected </{}>", tag)));
        }
        input.parse::<Token![>]>()?;
        children
    } else {
        Vec::new()
    };

    Ok(MViewNode::Element(MViewElement {
        tag,
        classes,
        id,
        attrs,
        events,
        children,
    }))
}

/// Parse attribute value: `"lit"` or `{expr}` or `|signal|`
fn parse_attr_value(input: ParseStream) -> Result<MViewAttrValue> {
    if input.peek(LitStr) {
        return Ok(MViewAttrValue::Static(input.parse()?));
    }
    if input.peek(Token![|]) {
        input.parse::<Token![|]>()?;
        let ident = input.call(Ident::parse_any)?;
        input.parse::<Token![|]>()?;
        return Ok(MViewAttrValue::SignalRef(ident.to_string()));
    }
    if input.peek(syn::token::Brace) {
        let content;
        syn::braced!(content in input);
        return Ok(MViewAttrValue::Reactive(content.parse()?));
    }
    if input.peek(syn::token::Paren) {
        let content;
        syn::parenthesized!(content in input);
        return Ok(MViewAttrValue::Reactive(content.parse()?));
    }
    // Bare expression
    Ok(MViewAttrValue::Reactive(input.parse()?))
}

/// Parse component: `ComponentName { prop=value, .. } { children }`
fn parse_component(input: ParseStream, name: Ident) -> Result<MViewNode> {
    let mut props = Vec::new();
    let mut children = Vec::new();

    if input.peek(syn::token::Brace) {
        let content;
        syn::braced!(content in input);
        while !content.is_empty() {
            let key_ident = content.call(Ident::parse_any)?;
            let key = key_ident.to_string();
            let value = if content.peek(Token![=]) {
                content.parse::<Token![=]>()?;
                parse_prop_value(&content)?
            } else {
                MViewPropValue::Bool(true)
            };
            props.push(MViewProp { name: key, value });
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }
    }

    if input.peek(syn::token::Brace) {
        let content;
        syn::braced!(content in input);
        children = content.parse::<MViewInput>()?.0;
    }

    Ok(MViewNode::Component { name, props, children })
}

/// Parse component prop value
fn parse_prop_value(input: ParseStream) -> Result<MViewPropValue> {
    if input.peek(LitStr) {
        return Ok(MViewPropValue::Static(input.parse()?));
    }
    if input.peek(Token![|]) {
        input.parse::<Token![|]>()?;
        let ident = input.call(Ident::parse_any)?;
        input.parse::<Token![|]>()?;
        return Ok(MViewPropValue::SignalRef(ident.to_string()));
    }
    if input.peek(syn::token::Brace) {
        let content;
        syn::braced!(content in input);
        return Ok(MViewPropValue::Reactive(content.parse()?));
    }
    Ok(MViewPropValue::Reactive(input.parse()?))
}

/// Parse `@if cond { then } @else { otherwise }`
fn parse_if(input: ParseStream) -> Result<MViewNode> {
    let cond: Expr = input.parse()?;
    let then_branch = parse_block(input)?;
    let otherwise = if input.peek(Token![@]) && input.peek2(Ident::peek_any) {
        let fork = input.fork();
        fork.parse::<Token![@]>()?;
        let ident = fork.call(Ident::parse_any)?;
        if ident.to_string() == "else" {
            input.advance_to(&fork);
            // Parse the else block directly (already consumed @else)
            Some(parse_block(input)?)
        } else {
            None
        }
    } else {
        None
    };
    Ok(MViewNode::If { cond, then: then_branch, otherwise })
}

/// Parse block `{ ... }`
fn parse_block(input: ParseStream) -> Result<Vec<MViewNode>> {
    let content;
    syn::braced!(content in input);
    Ok(content.parse::<MViewInput>()?.0)
}

/// Parse `@for pat in expr { body }`
fn parse_for(input: ParseStream) -> Result<MViewNode> {
    let pat = Pat::parse_single(input)?;
    input.parse::<Token![in]>()?;
    let expr = input.parse()?;
    let body = parse_block(input)?;
    Ok(MViewNode::For { pat, expr, body })
}

/// Parse `@let name = expr;`
fn parse_let(input: ParseStream) -> Result<MViewNode> {
    let pat = Pat::parse_single(input)?;
    input.parse::<Token![=]>()?;
    let value = input.parse()?;
    if input.peek(Token![;]) {
        input.parse::<Token![;]>()?;
    }
    let name = match pat {
        Pat::Ident(i) => i.ident,
        _ => return Err(syn::Error::new_spanned(pat, "@let expects a simple identifier")),
    };
    Ok(MViewNode::Let { name, value })
}

// --- Code generation ---

/// Generate `DomNode` code from MViewNode
fn gen_mview_node(node: &MViewNode) -> TokenStream2 {
    match node {
        MViewNode::Text(lit) => {
            let text = lit.value();
            quote! { veloasm::DomNode::text(#text) }
        }
        MViewNode::Reactive(expr) => {
            quote! { veloasm::DomNode::render_expression(move || veloasm::signal_value!(#expr)) }
        }
        MViewNode::Fragment(children) => {
            let children_code = children.iter().map(gen_mview_node);
            quote! {
                {
                    let frag = veloasm::DomNode::fragment();
                    #( frag.append(&#children_code); )*
                    frag
                }
            }
        }
        MViewNode::Element(el) => gen_element(el),
        MViewNode::If { cond, then, otherwise } => {
            let then_code: Vec<_> = then.iter().map(gen_mview_node).collect();
            let else_code = otherwise.as_ref().map(|o| {
                let c: Vec<_> = o.iter().map(gen_mview_node).collect();
                quote! { Some({ let f = veloasm::DomNode::fragment(); #(f.append(&#c);)* f }) }
            }).unwrap_or_else(|| quote! { None });
            quote! {
                veloasm::DomNode::render_expression(move || {
                    let cond = veloasm::signal_value!(#cond);
                    if cond {
                        let f = veloasm::DomNode::fragment();
                        #( f.append(&#then_code); )*
                        f
                    } else {
                        #else_code.unwrap_or_else(|| veloasm::DomNode::empty())
                    }
                })
            }
        }
        MViewNode::For { pat, expr, body } => {
            let body_code: Vec<_> = body.iter().map(gen_mview_node).collect();
            quote! {
                {
                    let list = #expr;
                    let frag = veloasm::DomNode::fragment();
                    for #pat in list {
                        #( frag.append(&#body_code); )*
                    }
                    frag
                }
            }
        }
        MViewNode::Let { name, value } => {
            quote! { let #name = #value; veloasm::DomNode::empty() }
        }
        MViewNode::Component { name, props, children } => {
            let component_ident = name.clone();
            let props_ident = syn::Ident::new(&format!("{}Props", name), proc_macro2::Span::call_site());
            let mut prop_tokens = Vec::new();
            for prop in props {
                let key = syn::Ident::new(&prop.name, proc_macro2::Span::call_site());
                let val = gen_prop_value(&prop.value);
                prop_tokens.push(quote! { #key: #val });
            }
            let children_code: Vec<_> = children.iter().map(gen_mview_node).collect();
            quote! {
                #component_ident(#props_ident {
                    #(#prop_tokens,)*
                    children: {
                        let frag = veloasm::DomNode::fragment();
                        #( frag.append(&#children_code); )*
                        Some(frag)
                    }
                })
            }
        }
        MViewNode::Spread(expr) => {
            quote! { #expr }
        }
    }
}

fn gen_prop_value(v: &MViewPropValue) -> TokenStream2 {
    match v {
        MViewPropValue::Static(lit) => quote! { #lit },
        MViewPropValue::Reactive(expr) => quote! { move || veloasm::signal_value!(#expr) },
        MViewPropValue::SignalRef(name) => {
            let lit = LitStr::new(name, proc_macro2::Span::call_site());
            quote! { move || veloasm::signal_value!(#lit) }
        }
        MViewPropValue::Bool(b) => quote! { #b },
    }
}

fn gen_element(el: &MViewElement) -> TokenStream2 {
    let tag = &el.tag;
    let tag_lit = LitStr::new(tag, proc_macro2::Span::call_site());

    let class_expr = if el.classes.is_empty() {
        quote! { "" }
    } else {
        let joined = el.classes.join(" ");
        quote! { #joined }
    };

    let id_expr = el.id.as_ref().map(|id| quote! { Some(#id) }).unwrap_or_else(|| quote! { None });

    let mut setup = Vec::new();

    for attr in &el.attrs {
        let name = &attr.name;
        if let Some(val) = &attr.value {
            match val {
                MViewAttrValue::Static(lit) => {
                    setup.push(quote! { parent_node.set_attr(#name, #lit); });
                }
                MViewAttrValue::Reactive(expr) => {
                    // Check if this is a class toggle (class:xxx)
                    if name.starts_with("class:") {
                        // For class toggles, pass the closure directly without signal_value wrapping
                        setup.push(quote! { parent_node.reactive_attribute(#name, #expr); });
                    } else {
                        let rv = crate::reactive_value(expr);
                        setup.push(quote! { parent_node.reactive_attribute(#name, #rv); });
                    }
                }
                MViewAttrValue::SignalRef(sig) => {
                    let lit = LitStr::new(sig, proc_macro2::Span::call_site());
                    setup.push(quote! { parent_node.reactive_attribute(#name, move || veloasm::signal_value!(#lit)); });
                }
                MViewAttrValue::Bool(b) => {
                    if *b {
                        setup.push(quote! { parent_node.set_attr(#name, ""); });
                    }
                }
            }
        } else {
            setup.push(quote! { parent_node.set_attr(#name, ""); });
        }
    }

    for event in &el.events {
        let name = &event.name;
        let handler = &event.handler;
        setup.push(quote! { parent_node.on(#name, #handler); });
    }

    let children_code: Vec<_> = el.children.iter().map(gen_mview_node).collect();

    quote! {
        {
            let parent_node = veloasm::DomNode::element(#tag_lit);
            if !#class_expr.is_empty() {
                parent_node.set_attr("class", #class_expr);
            }
            if let Some(id) = #id_expr {
                parent_node.set_attr("id", id);
            }
            #(#setup)*
            #( parent_node.append(&#children_code); )*
            parent_node
        }
    }
}

/// Public entry point for the mview macro
pub fn expand_mview(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as MViewInput);
    let code: Vec<_> = parsed.0.iter().map(gen_mview_node).collect();
    TokenStream::from(quote! {
        {
            #(#code)*
        }
    })
}
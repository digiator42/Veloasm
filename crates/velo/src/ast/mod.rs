//! # The client half of the shared AST
//!
//! `view!` compiles straight to Rust operating on [`DomNode`]. That is ideal for
//! a same-language client and useless for a backend that wants to *ship* a view.
//! This module closes the loop: it takes a [`velo_ast::View`] — the same data a
//! Rust backend built, serialized as JSON — and mounts it with the framework's
//! existing fine-grained primitives, so a button click rewrites one text node
//! and nothing else.
//!
//! ```text
//!   gritshield (native)                 browser (wasm)
//!   ───────────────────                 ─────────────────
//!   velo_ast::View  ──── JSON ────▶  veloast::ast::mount(view)
//!        │                                    │
//!        ├── render()  ──── HTML ────▶   (first paint, no JS)
//!        │                                    │
//!        └── handlers: ServerRoute             │
//!                    │                         ▼
//!                    └── POST /api/… ◀── event op dispatches here
//! ```
//!
//! ## The signal problem, and how this solves it
//!
//! A `velo_ast` view addresses state by *string* (`Expr::signal("count")`),
//! because that is what survives a round trip through JSON. The framework's own
//! signals are typed and unaddressable: [`RwSignal<T>`] is a `Copy` pointer into
//! a leaked `SignalEngine<T>`, so nothing can look one up by name.
//!
//! [`SignalStore`] is the bridge. It instantiates `RwSignal<Value>` — a real
//! typed handle over the *existing* engine, with no change to that engine — once
//! per declared signal and resolves ids to those handles. Everything downstream
//! is ordinary veloasm reactivity: `create_effect`, `batch`, scopes, disposal
//! on unmount. `Value` is JSON-shaped, so one signal can hold a whole table of
//! server records as easily as a counter.
//!
//! ## Loop bindings survive effect re-runs
//!
//! `Expr::LoopItem(d)` reads the row a keyed loop is rendering. An effect created
//! *inside* that row must keep reading the same row long after the render call
//! returned, so [`ResolverShim`] **snapshots** the loop stack when it is made
//! rather than consulting a shared one. Every effect the renderer creates
//! captures a shim taken at that moment, which is why nested bindings survive
//! independently.
//!
//! ## Lifetime caveat (inherited, not introduced)
//!
//! `RwSignal` engines are never freed — see [`crate::RwSignal`]. Mounting the same
//! view repeatedly therefore leaks one small engine per signal per mount. This
//! is exactly the trade the framework already makes for `signal!(…)` inside a
//! page body; mount views once per navigation, which is the intended usage.
//!
//! ## Entry points
//!
//! - [`mount`] — build a [`DomNode`] from a [`velo_ast::View`].
//! - [`Runtime`] — owns the signal store, handler table, and component table
//!   for one view.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use wasm_bindgen::{JsCast, JsValue};

use velo_ast::attr::{AttrValue, is_boolean_attribute};
use velo_ast::event::{EventAction, EventOp, EventValue, HandlerId};
use velo_ast::expr::{Expr, Resolver};
use velo_ast::path::ObjPath;
use velo_ast::node::{Element, NativeRenderFn, Node};
use velo_ast::value::Value;
use velo_ast::view::{HandlerRef, SignalDecl, SignalKind, View};

use crate::{create_effect, navigate_to, prefetch, request, scoped_node, DomNode, RwSignal};

// =============================================================================
// SignalStore — string ids -> reactive cells
// =============================================================================

/// One materialized reactive cell.
struct Cell {
    signal: RwSignal<Value>,
    kind: SignalKind,
    /// Soft cap for `List` signals; `None` is unbounded.
    cap: Option<usize>,
}

/// The reactive state behind one mounted view.
///
/// Reads go through [`ResolverShim::signal`], which calls `RwSignal::get()` — so
/// an expression that reaches a signal subscribes the running effect
/// automatically, with no explicit subscribe step anywhere in the renderer.
#[derive(Clone, Default)]
pub struct SignalStore {
    cells: Rc<RefCell<HashMap<String, Rc<Cell>>>>,
}

impl SignalStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Materialize every declaration in `view`.
    pub fn from_view(view: &View) -> Self {
        let store = SignalStore::new();
        for decl in &view.signals {
            store.insert(decl);
        }
        store
    }

    fn insert(&self, decl: &SignalDecl) {
        let mut cells = self.cells.borrow_mut();
        // First declaration wins, so an auto-declaration pass can never clobber
        // an explicit one (or real state written since mount).
        if cells.contains_key(&decl.id) {
            return;
        }
        cells.insert(
            decl.id.clone(),
            Rc::new(Cell {
                signal: RwSignal::new(decl.initial.clone()),
                kind: decl.kind,
                cap: decl.cap,
            }),
        );
    }

    /// Declare a signal at runtime — for a value the client wants to add to a
    /// backend-authored view.
    pub fn declare(&self, id: impl Into<String>, initial: Value, kind: SignalKind) {
        self.insert(&SignalDecl {
            id: id.into(),
            initial,
            kind,
            cap: None,
        });
    }

    /// The reactive handle behind `id` — the general escape hatch for handing
    /// view state to `effect!` or a hand-written closure.
    pub fn signal(&self, id: &str) -> Option<RwSignal<Value>> {
        self.cell(id).map(|c| c.signal)
    }

    fn cell(&self, id: &str) -> Option<Rc<Cell>> {
        self.cells.borrow().get(id).cloned()
    }

    /// Read a signal, subscribing the running effect.
    ///
    /// Unknown ids read as `Null` rather than panicking: a stale client must not
    /// take down a whole page.
    pub fn read(&self, id: &str) -> Value {
        match self.cell(id) {
            Some(c) => c.signal.get(),
            None => Value::Null,
        }
    }

    /// Write a signal, applying its declared `cap`. `false` if undeclared.
    pub fn write(&self, id: &str, value: Value) -> bool {
        let Some(c) = self.cell(id) else { return false };
        let stored = cap_value(value, c.kind, c.cap);
        c.signal.set(stored);
        true
    }

    /// Reset a signal to the empty value for its kind.
    pub fn clear(&self, id: &str) -> bool {
        let Some(c) = self.cell(id) else { return false };
        let empty = match c.kind {
            SignalKind::Scalar => Value::Null,
            SignalKind::List => Value::List(Vec::new()),
            SignalKind::Map => Value::Map(BTreeMap::new()),
        };
        c.signal.set(empty);
        true
    }

    /// Every declared id, sorted, for diagnostics.
    pub fn ids(&self) -> Vec<String> {
        let mut out: Vec<String> = self.cells.borrow().keys().cloned().collect();
        out.sort();
        out
    }

    /// Declared ids the view's tree reads but never declares — a build-time bug
    /// caught at mount instead of rendering as an empty cell forever.
    pub fn undeclared_reads(&self, view: &View) -> Vec<String> {
        view.undeclared_reads()
    }
}

/// Evict from the front so a log panel stays bounded without the view having to
/// think about it.
fn cap_value(value: Value, kind: SignalKind, cap: Option<usize>) -> Value {
    let (Some(limit), SignalKind::List) = (cap, kind) else {
        return value;
    };
    let Value::List(mut items) = value else { return value };
    if items.len() > limit {
        items.drain(..items.len() - limit);
    }
    Value::List(items)
}

// =============================================================================
// ResolverShim — what an effect captures
// =============================================================================

/// Everything an [`Expr`] needs to evaluate, captured so it can be moved into a
/// `'static` effect.
///
/// The loop stack is a **snapshot**: an effect created inside a keyed row must
/// keep reading that row after the render call returned, so it captures the
/// bindings that were live at creation rather than a shared stack that would
/// have moved on.
#[derive(Clone)]
pub struct ResolverShim {
    store: SignalStore,
    payload: Rc<BTreeMap<String, Value>>,
    path: Rc<String>,
    params: Rc<HashMap<String, String>>,
    query: Rc<HashMap<String, String>>,
    /// Loop bindings, outermost first. `LoopItem(0)` is the last entry.
    loops: Rc<Vec<(Value, usize)>>,
}

impl ResolverShim {
    /// A shim for the current render position, snapshotting the loop stack.
    pub fn snapshot(store: SignalStore) -> Self {
        let (path, params, query) = current_route();
        ResolverShim {
            store,
            payload: Rc::new(BTreeMap::new()),
            path: Rc::new(path),
            params: Rc::new(params),
            query: Rc::new(query),
            loops: Rc::new(LOOP_STACK.with(|s| s.borrow().clone())),
        }
    }

    /// Attach a view's static payload.
    pub fn with_payload(mut self, payload: &BTreeMap<String, Value>) -> Self {
        self.payload = Rc::new(payload.clone());
        self
    }

    /// Attach an explicit loop-binding snapshot.
    ///
    /// Used when a row is rendered: the bindings live on a thread-local for the
    /// duration of the render, and every effect created inside the row must
    /// capture *these* values rather than a shared stack that has moved on.
    pub(crate) fn snapshot_with(&self, loops: Vec<(Value, usize)>) -> Self {
        ResolverShim {
            loops: Rc::new(loops),
            ..self.clone()
        }
    }

    /// The signal store this shim reads.
    pub fn store(&self) -> &SignalStore {
        &self.store
    }
}

impl Resolver for ResolverShim {
    /// Reads go through `RwSignal::get()`, which subscribes the running effect.
    /// This single call is what makes the whole renderer fine-grained.
    fn signal(&self, id: &str) -> Option<Value> {
        Some(self.store.read(id))
    }

    fn payload(&self, path: &ObjPath) -> Option<Value> {
        let root = Value::Map((*self.payload).clone());
        if path.is_root() {
            Some(root)
        } else {
            root.path(path)
        }
    }

    fn route_param(&self, name: &str) -> Option<String> {
        self.params.get(name).cloned()
    }

    fn query(&self, name: &str) -> Option<String> {
        self.query.get(name).cloned()
    }

    fn route(&self) -> String {
        (*self.path).clone()
    }

    fn loop_item(&self, depth: u16) -> Option<Value> {
        self.bound(depth).map(|(v, _)| v.clone())
    }

    fn loop_index(&self, depth: u16) -> Option<usize> {
        self.bound(depth).map(|(_, i)| *i)
    }
}

impl ResolverShim {
    fn bound(&self, depth: u16) -> Option<&(Value, usize)> {
        let n = self.loops.len();
        n.checked_sub(depth as usize + 1)
            .and_then(|i| self.loops.get(i))
    }
}

thread_local! {
    /// Live loop bindings for the row currently rendering, outermost first.
    static LOOP_STACK: RefCell<Vec<(Value, usize)>> = const { RefCell::new(Vec::new()) };
}

/// Push a loop binding for the duration of one synchronous render.
struct LoopGuard;

impl LoopGuard {
    fn enter(item: Value, index: usize) -> Self {
        LOOP_STACK.with(|s| s.borrow_mut().push((item, index)));
        LoopGuard
    }
}

impl Drop for LoopGuard {
    fn drop(&mut self) {
        LOOP_STACK.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

/// The current route's path, params, and query, from the router.
fn current_route() -> (String, HashMap<String, String>, HashMap<String, String>) {
    let path = crate::AppRouter::use_route();
    let params = crate::AppRouter::params();
    let query = crate::CURRENT_QUERY.with(|q| q.get());
    (path, params, query)
}

/// Read the [`SeedIsland`](velo_ast::html::SeedIsland) emitted next to
/// server-rendered markup, so a client can adopt SSR'd state without a refetch.
///
/// `None` when the feature is off or the island is absent.
#[cfg(feature = "json")]
pub fn read_seed_island() -> Option<velo_ast::html::SeedIsland> {
    let el = crate::document().get_element_by_id("velo-view-seeds")?;
    let raw: web_sys::HtmlElement = el.dyn_into().ok()?;
    velo_ast::html::SeedIsland::from_json(&velo_ast::parse_html::unescape(&raw.inner_html()))
}

// =============================================================================
// Runtime — handlers and components for one view
// =============================================================================

/// What a handler receives when it runs.
pub struct EventCtx<'a> {
    /// The raw DOM event.
    pub event: &'a web_sys::Event,
    /// The decoded body of the most recent server call in this action.
    pub response: Option<Value>,
    /// Read/write view state.
    pub store: &'a SignalStore,
    /// The full AST, for handlers that want to read payloads.
    pub view: &'a View,
}

type HandlerFn = Rc<dyn Fn(&EventCtx)>;
type ComponentFn = Rc<dyn Fn(&[AttrValue], &[Node], &SignalStore) -> DomNode>;

/// Everything one mounted view owns besides its signals.
#[derive(Clone, Default)]
pub struct Runtime {
    store: SignalStore,
    handlers: Rc<RefCell<HashMap<String, HandlerRef>>>,
    local: Rc<RefCell<HashMap<String, HandlerFn>>>,
    components: Rc<RefCell<HashMap<String, ComponentFn>>>,
}

impl Runtime {
    /// Build a runtime for `view`, materializing its signals.
    pub fn new(view: &View) -> Self {
        Runtime {
            store: SignalStore::from_view(view),
            handlers: Rc::new(RefCell::new(
                view.handlers
                    .iter()
                    .map(|h| (h.id.to_string(), h.clone()))
                    .collect(),
            )),
            local: Rc::new(RefCell::new(HashMap::new())),
            components: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    /// This runtime's signal store.
    pub fn store(&self) -> &SignalStore {
        &self.store
    }

    /// Register a client-local handler.
    ///
    /// This is the escape hatch for behaviour that genuinely belongs in the
    /// browser — a keyboard shortcut, a `localStorage` sync. *Feature* logic
    /// should stay on the backend as a [`HandlerRef::remote`]; this exists for
    /// the residue that cannot.
    pub fn on(&self, name: impl Into<String>, f: impl Fn(&EventCtx) + 'static) {
        self.local.borrow_mut().insert(name.into(), Rc::new(f));
    }

    /// Register a component the view may reference via [`Node::Component`].
    pub fn component(
        &self,
        name: impl Into<String>,
        f: impl Fn(&[AttrValue], &[Node], &SignalStore) -> DomNode + 'static,
    ) {
        self.components.borrow_mut().insert(name.into(), Rc::new(f));
    }

    fn lookup_local(&self, name: &str) -> Option<HandlerFn> {
        self.local.borrow().get(name).cloned()
    }

    /// The runtime's copy of the handler table, which may override the view's.
    ///
    /// A registered [`Runtime::on`] takes precedence (checked first in
    /// `call_handler`); this lets a caller also *re-point* a declared handler at
    /// a different route without rebuilding the view.
    pub fn handler(&self, name: &str) -> Option<HandlerRef> {
        self.handlers.borrow().get(name).cloned()
    }

    fn lookup_component(&self, name: &str) -> Option<ComponentFn> {
        self.components.borrow().get(name).cloned()
    }
}

// =============================================================================
// mount / render
// =============================================================================

/// Mount `view` and return its root [`DomNode`].
///
/// Effects and listeners are scoped to the returned node, so unmounting it
/// disposes everything the view created — the same guarantee `#[component]`
/// gives.
///
/// ```ignore
/// let json = fetch("/api/panel").await?.text().await?;
/// let view = velo_ast::View::from_json(&json)?;
/// veloast::mount(veloast::ast::mount(&view));
/// ```
pub fn mount(view: &View) -> DomNode {
    let runtime = Runtime::new(view);
    scoped_node(|| render(view, &runtime))
}

/// Mount a bare [`Node`] tree and return its root [`DomNode`].
///
/// This is the entry point for `vhtml!`, which builds a tree rather than a
/// view — there is no wire format involved, and no signals to declare, because
/// the tree closes over real Rust signals directly.
///
/// ```ignore
/// use veloasm::{signal, vhtml};
///
/// let count = signal!(0u32);
/// let node = vhtml! { button { "clicked " (count) } };
/// veloasm::mount(veloasm::ast::mount_node(node));
/// ```
pub fn mount_node(root: Node) -> DomNode {
    mount(&View::new("vhtml", root))
}

/// Render `view` with a runtime you keep, so handlers, components, or extra
/// signals can be registered first.
pub fn render(view: &View, runtime: &Runtime) -> DomNode {
    let shim = ResolverShim::snapshot(runtime.store().clone()).with_payload(&view.payload);
    render_node(&view.root, view, runtime, &shim)
}

fn render_node(node: &Node, view: &View, rt: &Runtime, shim: &ResolverShim) -> DomNode {
    match node {
        Node::Missing => DomNode::empty(),

        Node::Text(expr) => {
            if expr.is_static() {
                // No signal, no effect, one text node.
                return DomNode::text(&expr.eval(shim).to_text());
            }
            let expr = expr.clone();
            let shim = shim.clone();
            DomNode::render_expression(move || expr.eval(&shim).to_text())
        }

        Node::Raw(html) => raw_fragment(html),

        Node::Fragment(children) => {
            let frag = DomNode::fragment();
            for child in children {
                let built = render_node(child, view, rt, shim);
                frag.append(&built);
            }
            frag
        }

        Node::If {
            test,
            then,
            otherwise,
        } => {
            if test.eval(shim).truthy() {
                render_node(then, view, rt, shim)
            } else {
                render_node(otherwise, view, rt, shim)
            }
        }

        Node::Show {
            when,
            children,
            fallback,
        } => {
            // Both branches build once and stay independently reactive;
            // `reactive_switch` moves whichever is active in and out of the DOM
            // so nested effects survive the swap.
            let content = render_node(children, view, rt, shim);
            let alt = render_node(fallback, view, rt, shim);
            let when = when.clone();
            let shim = shim.clone();
            crate::reactive_switch(
                move || when.eval(&shim).truthy(),
                content,
                alt,
            )
        }

        Node::Each { items, key, body } => render_each(items, key.as_ref(), body, view, rt, shim),

        Node::Component {
            name,
            attrs,
            children,
        } => match rt.lookup_component(name) {
            Some(f) => {
                let props: Vec<AttrValue> = attrs.iter().map(|a| a.value.clone()).collect();
                f(&props, children, rt.store())
            }
            None => {
                // Unknown component: an inert, inspectable placeholder rather
                // than a panic, so a stale client degrades visibly.
                let el = DomNode::element("velo-component");
                el.set_attr("data-name", name);
                el.set_attr("data-missing", "true");
                el
            }
        },

        Node::Slot { .. } => DomNode::empty(),

        Node::Native(build) => render_native(build, view, rt, shim),

        Node::Element(el) => render_element(el, view, rt, shim),
    }
}

/// A client-only subtree produced by arbitrary Rust, kept live.
///
/// The closure runs **inside** the effect, which is the whole point: a signal
/// read within it subscribes this effect, so the subtree is rebuilt whenever its
/// dependencies change. This is what lets `vhtml!`'s `@for` keep Maud's
/// plain-Rust loop body — including the loop variable — and still be reactive.
///
/// Each pass renders into a fresh scope and swaps it in, disposing the previous
/// subtree's effects. Rows are therefore rebuilt rather than reconciled; the
/// portable [`Node::Each`] path is the one that preserves DOM nodes across
/// updates.
fn render_native(
    build: &NativeRenderFn,
    view: &View,
    rt: &Runtime,
    shim: &ResolverShim,
) -> DomNode {
    let container = DomNode::display_contents();
    let container_raw = container.raw_node.clone();
    // Weak, to avoid res -> effect -> closure -> res ownership cycle.
    let container_res = Rc::downgrade(&container.res);
    let build = build.clone();
    let view = view.clone();
    let rt = rt.clone();
    let shim = shim.clone();

    let handle = create_effect(move || {
        let Some(res) = container_res.upgrade() else {
            return;
        };
        let built = build.call(&shim);
        let rendered = crate::scoped_node(|| render_node(&built, &view, &rt, &shim));

        // Drop ownership of the old children (disposing their effects) and
        // detach them, exactly as the dynamic-expression swap does.
        res.borrow_mut().children.clear();
        while let Some(child) = container_raw.first_child() {
            let _ = container_raw.remove_child(&child);
        }

        res.borrow_mut().absorb(&rendered.res);
        let _ = container_raw.append_child(&rendered.raw_node);
    });
    container.retain_effect(handle);
    container
}

/// Verbatim markup, adopted as a fragment.
///
/// `innerHTML` is the only way to get arbitrary markup into the DOM without a
/// parser, and [`Node::Raw`] is by contract "markup you produced yourself".
fn raw_fragment(html: &str) -> DomNode {
    let staging = DomNode::element("div");
    if let Ok(el) = staging.raw_node.clone().dyn_into::<web_sys::Element>() {
        el.set_inner_html(html);
        let out = DomNode::fragment();
        while let Some(child) = el.first_child() {
            let _ = el.remove_child(&child);
            let _ = out.raw_node.append_child(&child);
        }
        return out;
    }
    DomNode::empty()
}

// ---------------------------------------------------------------------------
// Elements
// ---------------------------------------------------------------------------

/// Every writer an element has for the `class` attribute, collected before
/// anything is written.
///
/// The framework's `reactive_classes` exists because a base `class="…"` and a
/// `class:toggle` both write the *same* attribute and would otherwise clobber
/// each other. An AST element can carry any number of either, so the decision
/// has to be made up front.
struct ClassPlan {
    base_static: String,
    base_reactive: Vec<Expr>,
    toggles: Vec<(String, Expr)>,
    present: bool,
}

fn plan_classes(el: &Element) -> ClassPlan {
    let mut plan = ClassPlan {
        base_static: String::new(),
        base_reactive: Vec::new(),
        toggles: Vec::new(),
        present: false,
    };
    for attr in &el.attrs {
        match &attr.value {
            AttrValue::ClassToggle { class, when } => {
                plan.present = true;
                plan.toggles.push((class.clone(), when.clone()));
            }
            AttrValue::Static(e) | AttrValue::Bound(e) if attr.name == "class" => {
                plan.present = true;
                match e.literal() {
                    // A literal base joins the static string; anything else
                    // needs an effect and so joins the reactive list.
                    Some(lit) => {
                        plan.base_static = if plan.base_static.is_empty() {
                            lit
                        } else {
                            format!("{} {lit}", plan.base_static)
                        };
                    }
                    None => plan.base_reactive.push(e.clone()),
                }
            }
            _ => {}
        }
    }
    plan
}

/// Apply the planned class writers so exactly one of them owns the attribute.
fn apply_classes(node: &DomNode, plan: ClassPlan, shim: &ResolverShim) {
    if !plan.present {
        return;
    }
    if plan.base_reactive.is_empty() {
        if plan.toggles.is_empty() {
            node.set_attr("class", &plan.base_static);
            return;
        }
        // The framework's coordinated path: one registry, every contributor
        // recorded into it, `className` rebuilt from the map.
        let toggles: Vec<(&'static str, Box<dyn FnMut() -> bool + 'static>)> = plan
            .toggles
            .into_iter()
            .map(|(name, when)| {
                let shim = shim.clone();
                let f: Box<dyn FnMut() -> bool + 'static> =
                    Box::new(move || when.eval(&shim).truthy());
                // `reactive_classes` keeps `&'static str` names for the life of
                // the node, so the class names are leaked deliberately rather
                // than adding a lifetime to the framework's signature.
                let leaked: &'static str = Box::leak(name.into_boxed_str());
                (leaked, f)
            })
            .collect();
        node.reactive_classes(&plan.base_static, toggles);
        return;
    }

    // A reactive base plus toggles must share ONE writer, or whichever effect
    // re-runs last wipes the other's classes.
    let ClassPlan {
        base_static,
        base_reactive,
        toggles,
        ..
    } = plan;
    let shim = shim.clone();
    node.reactive_attribute("class", move || {
        let mut parts: Vec<String> = Vec::new();
        if !base_static.is_empty() {
            parts.push(base_static.clone());
        }
        for b in &base_reactive {
            for c in b.eval(&shim).to_text().split_whitespace() {
                parts.push(c.to_string());
            }
        }
        for (name, when) in &toggles {
            if when.eval(&shim).truthy() {
                parts.push(name.clone());
            }
        }
        parts.join(" ")
    });
}

fn render_element(el: &Element, view: &View, rt: &Runtime, shim: &ResolverShim) -> DomNode {
    let node = DomNode::element(&el.tag);
    let classes = plan_classes(el);
    let mut static_style: Vec<(String, String)> = Vec::new();

    for attr in &el.attrs {
        match &attr.value {
            AttrValue::Ref(name) => node.set_attr("data-velo-ref", name),

            AttrValue::ClassToggle { .. } => {} // folded into apply_classes

            AttrValue::StyleProp { prop, value } => {
                if value.is_static() {
                    let v = value.eval(shim);
                    if v.truthy() {
                        static_style.push((prop.clone(), v.to_text()));
                    }
                } else {
                    let prop = prop.clone();
                    let value = value.clone();
                    let shim = shim.clone();
                    node.reactive_style(&prop, move || value.eval(&shim).to_text());
                }
            }

            AttrValue::Boolean(expr) => {
                let name = attr.name.clone();
                let raw = node.raw_node.clone();
                if expr.is_static() {
                    if expr.eval(shim).truthy() {
                        node.set_attr(&name, "");
                    }
                } else {
                    let expr = expr.clone();
                    let shim = shim.clone();
                    node.retain_effect(create_effect(move || {
                        let on = expr.eval(&shim).truthy();
                        if let Ok(el) = raw.clone().dyn_into::<web_sys::Element>() {
                            if on {
                                let _ = el.set_attribute(&name, "");
                            } else {
                                let _ = el.remove_attribute(&name);
                            }
                        }
                    }));
                }
            }

            AttrValue::BindValue(expr) => bind_value(&node, expr, shim),
            AttrValue::BindChecked(expr) => bind_checked(&node, expr, shim),

            AttrValue::Static(expr) => {
                if attr.name == "class" {
                    continue; // handled by apply_classes
                }
                match expr.literal() {
                    Some(text) => {
                        if is_boolean_attribute(&attr.name) {
                            if !text.is_empty() && text != "false" {
                                node.set_attr(&attr.name, "");
                            }
                        } else {
                            node.set_attr(&attr.name, &text);
                        }
                    }
                    // A non-literal in a Static slot still has to track.
                    None => bind_plain_attribute(&node, &attr.name, expr, shim),
                }
            }

            AttrValue::Bound(expr) => {
                if attr.name == "class" {
                    continue;
                }
                if expr.is_static() {
                    let v = expr.eval(shim);
                    if !matches!(v, Value::Null) {
                        node.set_attr(&attr.name, &v.to_text());
                    }
                } else {
                    bind_plain_attribute(&node, &attr.name, expr, shim);
                }
            }
        }
    }

    apply_classes(&node, classes, shim);

    if !static_style.is_empty() {
        let css: String = static_style
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("; ");
        node.set_attr("style", &css);
    }

    for binding in &el.events {
        attach_event(&node, &binding.event, &binding.action, view, rt, shim);
    }

    for child in &el.children {
        let built = render_node(child, view, rt, shim);
        node.append(&built);
    }

    node
}

/// A plain reactive attribute. `Null` clears the attribute rather than writing
/// an empty string — `title=""` and no `title` are different things to a screen
/// reader.
fn bind_plain_attribute(node: &DomNode, name: &str, expr: &Expr, shim: &ResolverShim) {
    let name = name.to_string();
    let expr = expr.clone();
    let shim = shim.clone();
    let boolean = is_boolean_attribute(name.as_str());
    let attr_name = name.clone();
    node.reactive_attribute(&name, move || {
        let v = expr.eval(&shim);
        if boolean {
            // A boolean attribute's *value* is its presence, so the effect
            // returns the name itself and `reactive_attribute` writes it.
            return if v.truthy() { attr_name.clone() } else { String::new() };
        }
        if matches!(v, Value::Null) {
            String::new()
        } else {
            v.to_text()
        }
    });
}

/// Two-way text binding: signal -> DOM on every change, DOM -> signal on `input`.
fn bind_value(node: &DomNode, expr: &Expr, shim: &ResolverShim) {
    let raw = node.raw_node.clone();

    // Signal -> element. The raw `Node` (a JS handle) is captured, never a
    // `DomNode` clone, so the effect cannot form a res -> effect -> res cycle.
    let expr_a = expr.clone();
    let shim_a = shim.clone();
    node.retain_effect(create_effect(move || {
        let text = expr_a.eval(&shim_a).to_text();
        if let Ok(el) = raw.clone().dyn_into::<web_sys::HtmlInputElement>() {
            // Only write when it differs: assigning `value` unconditionally
            // would move the caret to the end on every keystroke.
            if el.value() != text {
                let _ = el.set_value(&text);
            }
        }
    }));

    // The bound value still has to reach the DOM, so re-run it through the
    // attribute writer rather than duplicating the write path.
    bind_plain_attribute(node, "value", expr, shim);

    // Element -> signal. A binding to a computed expression (`a + b`) has no
    // single writable target, so the reverse direction is skipped rather than
    // writing to an arbitrary cell.
    let Some(target) = single_signal_id(expr) else { return };
    let store = shim.store.clone();
    node.on("input", move |e: web_sys::Event| {
        let Some(el) = e.target() else { return };
        let value = el
            .clone()
            .dyn_into::<web_sys::HtmlInputElement>()
            .map(|i| i.value())
            .or_else(|_| el.clone().dyn_into::<web_sys::HtmlTextAreaElement>().map(|t| t.value()))
            .or_else(|_| el.clone().dyn_into::<web_sys::HtmlSelectElement>().map(|s| s.value()))
            .unwrap_or_default();
        store.write(&target, Value::Str(value));
    });
}

/// Two-way checkbox binding.
fn bind_checked(node: &DomNode, expr: &Expr, shim: &ResolverShim) {
    let raw = node.raw_node.clone();
    let expr_a = expr.clone();
    let shim_a = shim.clone();
    node.retain_effect(create_effect(move || {
        let on = expr_a.eval(&shim_a).truthy();
        if let Ok(el) = raw.clone().dyn_into::<web_sys::HtmlInputElement>() {
            if el.checked() != on {
                let _ = el.set_checked(on);
            }
        }
    }));

    let Some(target) = single_signal_id(expr) else { return };
    let store = shim.store.clone();
    node.on("change", move |e: web_sys::Event| {
        let Some(el) = e.target() else { return };
        if let Ok(input) = el.dyn_into::<web_sys::HtmlInputElement>() {
            store.write(&target, Value::Bool(input.checked()));
        }
    });
}

/// The single signal an expression reads, if it reads exactly one.
fn single_signal_id(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Signal { id, path } if path.is_root() => Some(id.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

fn attach_event(
    node: &DomNode,
    event: &str,
    action: &EventAction,
    view: &View,
    rt: &Runtime,
    shim: &ResolverShim,
) {
    let action = action.clone();
    let view = view.clone();
    let runtime = rt.clone();
    let shim = shim.clone();
    let event_name = event.to_string();

    node.on(event, move |e: web_sys::Event| {
        if action.prevents_default() {
            e.prevent_default();
        }
        // Hover pre-warm, so a backend-authored `<a href=…>` gets `<Link
        // prefetch>` behaviour without the backend having to know about it.
        if event_name == "mouseenter" {
            if let Some(href) = e
                .current_target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .and_then(|el| el.get_attribute("href"))
            {
                prefetch(&href);
            }
        }
        let ctx = EventCtx {
            event: &e,
            response: None,
            store: runtime.store(),
            view: &view,
        };
        dispatch(&action.ops, &ctx, &view, &runtime, &shim);
    });
}

/// Run an op list.
///
/// Handler failures are swallowed on purpose: a failing action must not leave a
/// half-applied state or tear down the view.
///
/// The server reply is carried in a shared cell rather than returned: a remote
/// call is asynchronous, so the response cannot be sequenced synchronously with
/// the ops that follow it. The reply's real job — landing in the handler's
/// destination signal — happens regardless; `EventValue::Response` sees it if a
/// later await in the same action gets there first.
fn dispatch(ops: &[EventOp], ctx: &EventCtx, view: &View, rt: &Runtime, shim: &ResolverShim) {
    let shared: Rc<RefCell<Option<Value>>> = Rc::new(RefCell::new(ctx.response.clone()));
    for op in ops {
        let response = shared.borrow().clone();
        match op {
            EventOp::Set { target, value } => {
                let v = eval_event_value(value, ctx, shim, &response);
                ctx.store.write(target, v);
            }
            EventOp::Update { target, op, by } => {
                let rhs = eval_event_value(by, ctx, shim, &response);
                let cur = ctx.store.read(target);
                ctx.store.write(target, op.apply(&cur, &rhs));
            }
            EventOp::Toggle { target } => {
                let cur = ctx.store.read(target);
                // A numeric signal steps; anything else flips truthiness, so a
                // tri-state or object toggle stays expressible.
                let next = match cur {
                    Value::Num(n) => Value::Num(n + 1.0),
                    other => Value::Bool(!other.truthy()),
                };
                ctx.store.write(target, next);
            }
            EventOp::Push { target, value } => {
                let v = eval_event_value(value, ctx, shim, &response);
                // A blank input should not accumulate empty rows.
                if matches!(v, Value::Null) || matches!(&v, Value::Str(s) if s.is_empty()) {
                    continue;
                }
                let mut items = match ctx.store.read(target) {
                    Value::List(items) => items,
                    _ => Vec::new(),
                };
                items.push(v);
                ctx.store.write(target, Value::List(items));
            }
            EventOp::Clear { target } => {
                ctx.store.clear(target);
            }
            EventOp::Navigate { to } => {
                let dest = to.eval(shim).to_text();
                if !dest.is_empty() {
                    navigate_to(&dest);
                }
            }
            EventOp::AllowDefault => {}
            EventOp::Debounce { ms, ops } => {
                schedule_debounced(*ms, ops.clone(), view.clone(), rt.clone(), shim.clone());
            }
            EventOp::Call { handler } => {
                call_handler(handler, ctx, view, rt, shim, &shared);
            }
        }
    }
}

fn eval_event_value(
    value: &EventValue,
    ctx: &EventCtx,
    shim: &ResolverShim,
    response: &Option<Value>,
) -> Value {
    match value {
        EventValue::Expr(e) => e.eval(shim),
        EventValue::TargetProperty(name) => ctx
            .event
            .target()
            .and_then(|t| read_dom_property(&t, name))
            .unwrap_or(Value::Null),
        EventValue::TargetText => ctx
            .event
            .target()
            .and_then(|t| {
                t.clone()
                    .dyn_into::<web_sys::HtmlInputElement>()
                    .ok()
                    .map(|i| i.value())
                    .or_else(|| t.dyn_into::<web_sys::Node>().ok().and_then(|n| n.text_content()))
            })
            .map(Value::Str)
            .unwrap_or(Value::Null),
        EventValue::Response => response.clone().unwrap_or(Value::Null),
    }
}

/// Read a DOM property off an event target as a `Value`.
fn read_dom_property(target: &web_sys::EventTarget, name: &str) -> Option<Value> {
    let element = target.clone().dyn_into::<web_sys::Element>().ok()?;
    let input = target.clone().dyn_into::<web_sys::HtmlInputElement>().ok();
    match name {
        "value" => input
            .map(|i| Value::Str(i.value()))
            .or_else(|| {
                target
                    .clone()
                    .dyn_into::<web_sys::HtmlTextAreaElement>()
                    .ok()
                    .map(|t| Value::Str(t.value()))
            })
            .or_else(|| {
                target
                    .clone()
                    .dyn_into::<web_sys::HtmlSelectElement>()
                    .ok()
                    .map(|s| Value::Str(s.value()))
            })
            .or_else(|| element.get_attribute("value").map(Value::Str)),
        "checked" => input.map(|i| Value::Bool(i.checked())),
        "text" => element.text_content().map(Value::Str),
        other => element.get_attribute(other).map(Value::Str),
    }
}

/// Invoke a handler by id.
///
/// Order matters: a client-local registration is the most specific answer, so it
/// wins over the view's own declaration. Otherwise a handler with a
/// [`HandlerRef::remote`] route is issued as a request — the "authored on the
/// backend" path, where no view logic ever reaches the browser.
fn call_handler(
    handler: &HandlerId,
    ctx: &EventCtx,
    view: &View,
    rt: &Runtime,
    shim: &ResolverShim,
    response: &Rc<RefCell<Option<Value>>>,
) {
    if let Some(f) = rt.lookup_local(handler.as_str()) {
        let local = EventCtx {
            event: ctx.event,
            response: response.borrow().clone(),
            store: ctx.store,
            view,
        };
        f(&local);
        return;
    }

    let declared = rt
        .handler(handler.as_str())
        .or_else(|| view.handler(handler.as_str()).cloned());
    let (route, dest) = match declared {
        Some(h) => match h.server {
            Some(route) => (route, h.writes),
            None => {
                // Declared as local but not registered: the app author owns
                // that, so this is a silent no-op by design rather than an error.
                return;
            }
        },
        None => return,
    };

    let store = ctx.store.clone();
    let response = response.clone();
    let url = route.url(shim);
    let method = route.method.clone();
    let body = route.body.as_ref().map(|b| b.eval(shim).to_text());

    wasm_bindgen_futures::spawn_local(async move {
        let mut builder = request(&url).method(&method);
        // Send the body as *text*. Parsing it into a JS object first looks
        // tidier, but `RequestInit::set_body` takes a `BodyInit` — a string, a
        // Blob, a buffer — and any other value is coerced with `String()`, so a
        // plain object arrives at the server as the literal `[object Object]`.
        // The handler then reads an empty body and the request silently does
        // nothing.
        if let Some(text) = &body {
            builder = builder.header("content-type", "application/json");
            builder = builder.body(JsValue::from_str(text));
        }

        match builder.send().await {
            Ok(resp) => {
                let raw = resp.text().await.unwrap_or_default();
                let parsed = decode_reply(&raw);
                if let (Some(sig), Some(v)) = (dest, parsed.clone()) {
                    store.write(&sig, v);
                }
                *response.borrow_mut() = parsed;
            }
            Err(_) => {
                // A failed server call leaves existing state untouched and
                // reports nothing: an action that silently fails is better than
                // one that blanks a dashboard.
            }
        }
    });
}

/// Decode a server reply into a `Value`.
///
/// Without the `json` feature the AST runtime does not link `serde`, so a reply
/// arrives as an opaque string and is surfaced as one. Turn `json` on to get
/// typed replies — that is the only reason this is feature-gated rather than
/// hard-wired.
#[cfg(feature = "json")]
fn decode_reply(raw: &str) -> Option<Value> {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .map(|v| Value::from_json(&v))
}

#[cfg(not(feature = "json"))]
fn decode_reply(raw: &str) -> Option<Value> {
    (!raw.trim().is_empty()).then(|| Value::Str(raw.to_string()))
}

/// Run `ops` after `ms` of quiet.
fn schedule_debounced(ms: u32, ops: Vec<EventOp>, view: View, rt: Runtime, shim: ResolverShim) {
    wasm_bindgen_futures::spawn_local(async move {
        crate::sleep(ms).await;
        let Ok(event) = web_sys::Event::new("velo-debounced") else {
            return;
        };
        let ctx = EventCtx {
            event: &event,
            response: None,
            store: rt.store(),
            view: &view,
        };
        dispatch(&ops, &ctx, &view, &rt, &shim);
    });
}

// ---------------------------------------------------------------------------
// Loops
// ---------------------------------------------------------------------------

/// Render a repetition.
///
/// With a `key`, a row is reused when both its key *and* its bound item are
/// unchanged, so an append costs one node and a delete costs one node. With no
/// key, rows are positional and every change rebuilds them — the honest cost of
/// positional identity.
fn render_each(
    items: &Expr,
    key: Option<&Expr>,
    body: &Node,
    view: &View,
    rt: &Runtime,
    shim: &ResolverShim,
) -> DomNode {
    let container = DomNode::display_contents();
    let items_expr = items.clone();
    let outer_shim = shim.clone();
    let rows: Rc<RefCell<Vec<(Value, Value, DomNode)>>> = Rc::new(RefCell::new(Vec::new()));

    match key {
        Some(key_expr) => {
            let key_expr = key_expr.clone();
            let handle = create_effect({
                let rows = rows.clone();
                let body = body.clone();
                let view = view.clone();
                let rt = rt.clone();
                let container = container.clone();
                move || reconcile(
                    &container,
                    &items_expr,
                    Some(&key_expr),
                    &body,
                    &view,
                    &rt,
                    &outer_shim,
                    &rows,
                )
            });
            container.retain_effect(handle);
        }
        None => {
            let handle = create_effect({
                let rows = rows.clone();
                let body = body.clone();
                let view = view.clone();
                let rt = rt.clone();
                let container = container.clone();
                move || reconcile(
                    &container,
                    &items_expr,
                    None,
                    &body,
                    &view,
                    &rt,
                    &outer_shim,
                    &rows,
                )
            });
            container.retain_effect(handle);
        }
    }

    container
}

/// One reconciliation pass. Shared by the keyed and unkeyed paths; `key` is
/// `None` for the positional case, where nothing can be reused.
#[allow(clippy::too_many_arguments)]
fn reconcile(
    container: &DomNode,
    items: &Expr,
    key: Option<&Expr>,
    body: &Node,
    view: &View,
    rt: &Runtime,
    shim: &ResolverShim,
    rows: &Rc<RefCell<Vec<(Value, Value, DomNode)>>>,
) {
    let list = match items.eval(shim) {
        Value::List(items) => items,
        _ => Vec::new(),
    };

    // Compute each row's identity and value outside any loop binding: the key
    // expression is written against the row's own fields, so it needs the
    // binding pushed just for its own evaluation.
    let mut next: Vec<(Value, Value)> = Vec::with_capacity(list.len());
    for (index, item) in list.iter().enumerate() {
        let k = match key {
            Some(k) => eval_keyed(k, item, index, shim),
            None => Value::Num(index as f64),
        };
        next.push((k, item.clone()));
    }

    let mut kept = rows.borrow_mut();

    // Match survivors by **key**, not by position. Position matching would
    // rebuild every row after an insertion at the front — exactly the case
    // keyed reconciliation exists to avoid.
    //
    // A row survives only when its key *and* its bound value are unchanged: a
    // stable key with changed data must rebuild, or the row would keep showing
    // stale fields.
    let mut survivor: Vec<Option<usize>> = vec![None; next.len()];
    let mut consumed = vec![false; kept.len()];
    for (i, (k, item)) in next.iter().enumerate() {
        for (j, (rk, rv, _)) in kept.iter().enumerate() {
            if !consumed[j] && rk.identical(k) && rv.identical(item) {
                consumed[j] = true;
                survivor[i] = Some(j);
                break;
            }
        }
    }

    // Rows that are gone leave the DOM *and* release their resources, so
    // their effects are disposed with them.
    for (j, alive) in consumed.iter().enumerate() {
        if !alive {
            let node = &kept[j].2;
            if let Some(parent) = node.raw_node.parent_node() {
                let _ = parent.remove_child(&node.raw_node);
            }
            container.release(node);
        }
    }

    // Build or reuse each row, then place the rows in order.
    let mut rebuilt: Vec<(Value, Value, DomNode)> = Vec::with_capacity(next.len());
    let mut ordered: Vec<DomNode> = Vec::with_capacity(next.len());
    for (index, (k, item)) in next.iter().enumerate() {
        let node = match survivor[index] {
            Some(j) => kept[j].2.clone(),
            None => render_row(body, item, index, view, rt, shim),
        };
        ordered.push(node.clone());
        rebuilt.push((k.clone(), item.clone(), node));
    }

    // Place the rows with the fewest moves possible: a row is moved only when
    // it is not already immediately after its predecessor. Moving a node that
    // is already in place would still cost a DOM operation and would drop any
    // focus or selection state the user had inside it.
    let raw = container.raw_node.clone();
    let mut previous: Option<web_sys::Node> = None;
    for node in &ordered {
        let reference = previous.as_ref().and_then(|p| p.next_sibling());
        let already_there = match (&reference, &previous) {
            (Some(r), Some(_)) => node.raw_node.is_same_node(Some(r)),
            _ => false,
        };
        if !already_there {
            let _ = raw.insert_before(&node.raw_node, reference.as_ref());
        }
        previous = Some(node.raw_node.clone());
    }

    *kept = rebuilt;
}

/// Evaluate `f` with `item` bound as the innermost loop slot.
fn eval_keyed(
    key: &Expr,
    item: &Value,
    index: usize,
    outer: &ResolverShim,
) -> Value {
    let _guard = LoopGuard::enter(item.clone(), index);
    let shim = outer.snapshot_with(LOOP_STACK.with(|s| s.borrow().clone()));
    key.eval(&shim)
}

/// Build one row inside its own loop binding and effect scope.
///
/// `scoped_node` is what makes the row's effects die with the row: a row removed
/// from the list disposes everything it created.
fn render_row(
    body: &Node,
    item: &Value,
    index: usize,
    view: &View,
    rt: &Runtime,
    shim: &ResolverShim,
) -> DomNode {
    let _guard = LoopGuard::enter(item.clone(), index);
    let row_shim = shim.snapshot_with(LOOP_STACK.with(|s| s.borrow().clone()));
    scoped_node(|| render_node(body, view, rt, &row_shim))
}
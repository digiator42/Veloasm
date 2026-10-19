//! Element attributes: one ordered list per element, each entry tagged with
//! *how* it is applied.
//!
//! The original draft of this AST modelled attributes as
//! `HashMap<String, String>`. That loses three things the renderers need, so
//! the tag lives in the value instead:
//!
//! - **order** — a `Vec` makes SSR output byte-stable and diffable;
//! - **kind** — `disabled={ false }` must remove the attribute, not write the
//!   string `"false"`;
//! - **coherence** — `class="a"` and `class:b={ flag }` are *different writers*
//!   of the same attribute, and the runtime has to be able to tell so it can
//!   merge them instead of letting them clobber each other.

use crate::expr::Expr;

/// A single attribute binding on an element.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Attr {
    /// The attribute name as it appears in HTML (`class`, `href`, `data-x`).
    pub name: String,
    /// How the value is computed and applied.
    pub value: AttrValue,
}

impl Attr {
    /// Build an attribute with an explicit [`AttrValue`].
    pub fn new(name: impl Into<String>, value: AttrValue) -> Self {
        Attr {
            name: name.into(),
            value,
        }
    }

    /// A static one-shot attribute: `class="card"`.
    pub fn stat(name: impl Into<String>, value: impl Into<String>) -> Self {
        Attr::new(name, AttrValue::Static(Expr::str(value)))
    }

    /// A reactive attribute: `title={ tip }`.
    ///
    /// The name may carry a namespace prefix that the runtime interprets —
    /// `on:` (event), `class:` (toggle), `style:` (property), `bind:` (two-way),
    /// `ref:` (element handle) — or be a plain attribute written through
    /// `setAttribute`.
    pub fn bound(name: impl Into<String>, value: Expr) -> Self {
        Attr::new(name, AttrValue::Bound(value))
    }

    /// A boolean attribute: present when truthy, absent when falsy.
    pub fn boolean(name: impl Into<String>, value: Expr) -> Self {
        Attr::new(name, AttrValue::Boolean(value))
    }

    /// A class toggle: `class:is-active={ selected }`.
    ///
    /// Toggles are additive — they add or remove exactly one class name and
    /// never rewrite the whole `class` attribute — so any number of them can
    /// coexist with a `class="…"` base.
    pub fn class_toggle(class: impl Into<String>, when: Expr) -> Self {
        let class = class.into();
        Attr::new(
            format!("class:{class}"),
            AttrValue::ClassToggle { class, when },
        )
    }

    /// An inline style property: `style:color={ colour }`.
    pub fn style_prop(prop: impl Into<String>, value: Expr) -> Self {
        let prop = prop.into();
        Attr::new(
            format!("style:{prop}"),
            AttrValue::StyleProp { prop, value },
        )
    }

    /// A two-way text binding: `bind:value={ name }`.
    pub fn bind_value(value: Expr) -> Self {
        Attr::new("bind:value", AttrValue::BindValue(value))
    }

    /// A two-way checkbox binding: `bind:checked={ agreed }`.
    pub fn bind_checked(value: Expr) -> Self {
        Attr::new("bind:checked", AttrValue::BindChecked(value))
    }

    /// Name the element so handler code can look it up: `ref={ "email" }`.
    pub fn reference(name: impl Into<String>) -> Self {
        Attr::new("ref", AttrValue::Ref(name.into()))
    }

    /// `true` when applying this attribute requires an effect rather than a
    /// one-shot DOM write. Static attributes skip the effect machinery entirely,
    /// which is what keeps a 400-row admin table cheap.
    pub fn is_reactive(&self) -> bool {
        match &self.value {
            AttrValue::Static(e) => !e.is_static(),
            AttrValue::Bound(e)
            | AttrValue::Boolean(e)
            | AttrValue::ClassToggle { when: e, .. }
            | AttrValue::StyleProp { value: e, .. }
            | AttrValue::BindValue(e)
            | AttrValue::BindChecked(e) => !e.is_static(),
            AttrValue::Ref(_) => false,
        }
    }

    /// The expression this attribute reads, if any. Event bindings do not carry
    /// one — see [`crate::event::EventBinding`].
    pub fn expr(&self) -> Option<&Expr> {
        match &self.value {
            AttrValue::Static(e)
            | AttrValue::Bound(e)
            | AttrValue::Boolean(e)
            | AttrValue::BindValue(e)
            | AttrValue::BindChecked(e) => Some(e),
            AttrValue::ClassToggle { when, .. } | AttrValue::StyleProp { value: when, .. } => {
                Some(when)
            }
            AttrValue::Ref(_) => None,
        }
    }

    /// `true` if this is an event binding (`on:…`) rather than a plain attribute.
    pub fn is_event(&self) -> bool {
        self.name.starts_with("on:")
    }
}

/// How one attribute is computed and applied.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "k", content = "v"))]
pub enum AttrValue {
    /// Written once. A static expression is applied with a direct
    /// `setAttribute`; a dynamic one forces the reactive path.
    Static(Expr),
    /// Bound to a signal: re-evaluated on every change of its dependencies.
    Bound(Expr),
    /// A boolean attribute: truthy writes the empty string, falsy removes it.
    Boolean(Expr),
    /// `class:{class}={ when }` — additive class toggling.
    ClassToggle {
        /// The class name to add or remove.
        class: String,
        /// Condition controlling membership.
        when: Expr,
    },
    /// `style:{prop}={ value }` — one inline style property.
    StyleProp {
        /// The CSS property name, e.g. `color` or `--accent`.
        prop: String,
        /// The property value.
        value: Expr,
    },
    /// `bind:value={ sig }` — writes the DOM value into the signal on `input`
    /// and pushes signal changes back into the element.
    BindValue(Expr),
    /// `bind:checked={ sig }` — the checkbox equivalent.
    BindChecked(Expr),
    /// `ref={ name }` — publish this element to the view's element registry so
    /// event handlers and [`Node::Each`] keys can address it by name.
    Ref(String),
}

/// Attribute names the renderers treat as boolean (presence-only). Anything
/// listed here renders as `attr` or nothing, never `attr="false"`.
pub const BOOLEAN_ATTRIBUTES: &[&str] = &[
    "allowfullscreen",
    "async",
    "autofocus",
    "autoplay",
    "checked",
    "controls",
    "default",
    "defer",
    "disabled",
    "formnovalidate",
    "hidden",
    "inert",
    "ismap",
    "itemscope",
    "loop",
    "multiple",
    "muted",
    "nomodule",
    "novalidate",
    "open",
    "playsinline",
    "readonly",
    "required",
    "reversed",
    "selected",
];

/// Is `name` an HTML boolean attribute?
pub fn is_boolean_attribute(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    BOOLEAN_ATTRIBUTES.contains(&lower.as_str())
}

/// Elements that never have a closing tag. The HTML parser needs this to build a
/// correct tree, and the SSR renderer uses it to avoid emitting `</br>`.
pub const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Is `tag` a void element?
pub fn is_void_element(tag: &str) -> bool {
    VOID_ELEMENTS.contains(&tag.to_ascii_lowercase().as_str())
}

/// Elements whose content is raw text, not markup. `<script>` and `<style>`
/// bodies must never be entity-escaped or tag-parsed.
pub const RAW_TEXT_ELEMENTS: &[&str] = &["script", "style"];

/// Is `tag` a raw-text element?
pub fn is_raw_text_element(tag: &str) -> bool {
    RAW_TEXT_ELEMENTS.contains(&tag.to_ascii_lowercase().as_str())
}
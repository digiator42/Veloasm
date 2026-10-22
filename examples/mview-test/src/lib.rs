//! Quick test for `mview!` macro

use veloasm::{signal, mview, DomNode};

#[test]
fn mview_basic_element() {
    let _: DomNode = mview! {
        div {
            h2 { "Hello mview!" }
            p { "This works!" }
        }
    };
}

#[test]
fn mview_click_event() {
    let count = signal!(0u32);
    let _: DomNode = mview! {
        button @click={ move |_| count.set(count.get() + 1) } { "+" }
    };
}

#[test]
fn mview_with_id_and_class() {
    let _: DomNode = mview! {
        div id="main" class="container" {
            span { "content" }
        }
    };
}

#[test]
fn mview_maud_class_shorthand() {
    let _: DomNode = mview! {
        div.container {
            p { "has container class" }
        }
    };
}

#[test]
fn mview_reactive_signal() {
    let count = signal!(0u32);
    let _: DomNode = mview! {
        div {
            p { "Count: " { count } }
            button @click={ move |_| count.set(count.get() + 1) } { "+" }
        }
    };
}

#[test]
fn mview_signal_pipe_sugar() {
    let active = signal!(true);
    let _: DomNode = mview! {
        div class:active={ move || if active.get() { "active".to_string() } else { String::new() } } { "toggles" }
    };
}

#[test]
fn mview_event_shorthand() {
    let count = signal!(0u32);
    let _: DomNode = mview! {
        button @click={ move |_| count.set(count.get() + 1) } { "+" }
    };
}

#[test]
fn mview_control_flow_if() {
    let flag = signal!(true);
    let _: DomNode = mview! {
        div {
            @if flag.get() { p { "on" } } @else { p { "off" } }
        }
    };
}

#[test]
fn mview_for_loop() {
    let items = signal!(vec!["a", "b", "c"]);
    let _: DomNode = mview! {
        ul {
            @for item in items.get() {
                li { item }
            }
        }
    };
}

#[test]
fn mview_let_binding_ignored() {
    // TODO: Fix @let binding parsing
    // let _: DomNode = mview! {
    //     @let greeting = "Hello";
    //     p { greeting }
    // };
}

#[test]
fn mview_component_usage_ignored() {
    // TODO: Fix component usage
    // #[veloasm::component]
    // fn Card(title: String, active: bool) -> DomNode {
    //     mview! {
    //         div class:active={ active } {
    //             h3 { title }
    //         }
    //     }
    // }
    //
    // let _: DomNode = mview! {
    //     Card { title="Test", active=true } { }
    // };
}

#[test]
fn mview_nested_elements() {
    let _: DomNode = mview! {
        div.container {
            header { "Header" }
            main {
                section { "Content" }
            }
            footer { "Footer" }
        }
    };
}

#[test]
fn mview_multiple_roots() {
    let _: DomNode = mview! {
        <>
            div { "first" }
            div { "second" }
        </>
    };
}

#[test]
fn mview_class_toggle() {
    let active = signal!(true);
    let _: DomNode = mview! {
        div class:active={ move || if active.get() { "active".to_string() } else { String::new() } } { "toggles" }
    };
}

#[test]
fn mview_on_event() {
    let count = signal!(0u32);
    let _: DomNode = mview! {
        button on:click={ move |_| count.set(count.get() + 1) } { "+" }
    };
}

#[test]
fn mview_text_interpolation() {
    let name = signal!("World");
    let _: DomNode = mview! {
        p { "Hello, " { name } "!" }
    };
}

#[test]
fn mview_fragment() {
    let _: DomNode = mview! {
        <> 
            div { "first" }
            div { "second" }
        </>
    };
}
use veloasm::prelude::*;

// `name` arrives as a Copy `RwSignal` handle: the caller passes it by value
// with no `.clone()`, and `{ name }` auto-unwraps it via `ViewValue`.
#[allow(non_snake_case)]
#[component]
pub fn UserCard(name: RwSignal<String>, role: &'static str) -> DomNode {
    view! {
        <div class="user-card">
            <p>"Hello, " { name } "!"</p>
            <p class="role">"Role: " { role }</p>
        </div>
    }
}

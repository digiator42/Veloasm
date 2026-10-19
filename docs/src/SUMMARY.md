# Summary

[Introduction](introduction.md)

# Getting Started
- [Installation & Toolchain](getting-started/installation.md)
- [Quickstart Guide](getting-started/quickstart.md)
- [Project Structure](getting-started/project-structure.md)
- [Dev Server, Error Overlay & HMR](getting-started/dev-server-and-hmr.md)

# Reactivity Engine
- [Reactivity Overview & Mental Model](reactivity/overview.md)
- [Signals (Split Read & Write)](reactivity/signals.md)
- [RwSignal & Ergonomic Handles](reactivity/rw-signals.md)
- [Derived State with Memos](reactivity/memos.md)
- [Side Effects](reactivity/effects.md)
- [Update Batching](reactivity/batching.md)
- [Reactive Collections (SignalVec)](reactivity/signal-vec.md)

# The `view!` Macro & Templating
- [Macro Overview & Syntax](view-macro/overview.md)
- [Elements, Text & Auto-Unwrapping](view-macro/elements-and-text.md)
- [Reactive Attributes](view-macro/reactive-attributes.md)
- [Class & Style Toggles](view-macro/class-and-style.md)
- [Event Handling](view-macro/events.md)
- [Two-Way Form Bindings (`bind:`)](view-macro/two-way-binding.md)
- [Conditional Rendering & Control Flow](view-macro/control-flow.md)
- [Keyed Reactive Lists](view-macro/keyed-lists.md)

# The Shared AST (Server-Authored Views)
- [Shared AST Overview](shared-ast/overview.md)

# Components & Composition
- [Defining Components with `#[component]`](components/defining-components.md)
- [Component Props & Reactivity](components/props.md)
- [Children & Document Fragments](components/children-and-fragments.md)

# State Management
- [Context API (`provide_context` & `use_context`)](state-management/context-api.md)
- [Global Application State Stores](state-management/global-stores.md)

# Client-Side Routing
- [Routing Overview](routing/overview.md)
- [Router & Route Configuration](routing/router-and-routes.md)
- [Layouts & Nesting](routing/layouts-and-nesting.md)
- [Links & Client-Side Navigation](routing/links-and-navigation.md)
- [Route Parameters & Query Strings](routing/parameters-and-queries.md)
- [Suspense & Loading States](routing/suspense-and-loading.md)
- [Fetching Data & Prefetching](routing/fetching-data-and-prefetching.md)
- [Lazy Loading & Code Splitting](routing/code-splitting-and-lazy-loading.md)

# DOM & Lifecycle
- [Mounting the Application](dom-and-lifecycle/mounting.md)
- [Effect Lifecycle & Resource Cleanup](dom-and-lifecycle/effect-cleanup.md)
- [The `DomNode` API](dom-and-lifecycle/dom-node-api.md)
- [Timers, SSE & Streams](dom-and-lifecycle/timers-and-streams.md)
- [Error Boundaries](dom-and-lifecycle/error-boundaries.md)

# Best Practices & Architecture
- [Zero-Clone Ergonomics & Ownership](best-practices/zero-clone-patterns.md)
- [Performance & WASM Binary Size](best-practices/performance-and-sizing.md)
- [Troubleshooting & Common Gotchas](best-practices/debugging-and-troubleshooting.md)

# Walkthrough Examples
- [Counter SPA](examples/counter.md)
- [Task Manager (Todo App)](examples/todo-app.md)
- [High-Frequency Live Dashboard](examples/realtime-dashboard.md)

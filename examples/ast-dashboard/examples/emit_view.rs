//! Host-side tool for the `ast-dashboard` example.
//!
//! ```sh
//! # Emit the view payload the browser fetches, and report on it.
//! cargo run -p ast-dashboard --bin ast-dashboard -- --emit-view
//!
//! # Or just print the SSR markup.
//! cargo run -p ast-dashboard --bin ast-dashboard -- --print-html
//! ```

fn main() {
    let mut emit_view = false;
    let mut print_html = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--emit-view" => emit_view = true,
            "--print-html" => print_html = true,
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    if emit_view {
        // `public/` is where Trunk serves static assets from.
        let dir = std::path::Path::new("public");
        std::fs::create_dir_all(dir).expect("create public/");
        let path = dir.join("view.json");
        std::fs::write(&path, ast_dashboard::view_json()).expect("write view.json");
        println!("wrote {}", path.display());
    }

    if print_html {
        println!("{}", ast_dashboard::view_html());
    }

    if !emit_view && !print_html {
        println!("ast-dashboard — the shared AST, end to end\n");
        for (key, value) in ast_dashboard::summary() {
            println!("  {key:<12} {value}");
        }
        println!("\n  --emit-view    write public/view.json for the browser to fetch");
        println!("  --print-html   print the server-rendered markup");
    }
}

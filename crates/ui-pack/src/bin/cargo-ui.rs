//! `cargo ui <command>`: Cargo runs `cargo-ui ui <command> …`, so drop the `ui`.

fn main() {
    let mut args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("ui") {
        args.remove(1);
    }
    args[0] = "cargo ui".into();
    ui_pack::cli::main_with(args);
}

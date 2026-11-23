//! `ui-pack` CLI for packaging and running `ui` desktop applications.

fn main() {
    ui_pack::cli::main_with(std::env::args().collect());
}

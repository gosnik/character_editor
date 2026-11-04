//! Compatibility entry point: original export flags and the complete headless CLI.
fn main() {
    std::process::exit(human_editor::cli::legacy_entry(std::env::args_os()));
}

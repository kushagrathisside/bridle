//! `agent`: the harness command-line interface. Wiring only; logic lives in the library crates.

fn main() {
    println!("{} — M1 scaffold", harness_core::identity::CLI_NAME);
}

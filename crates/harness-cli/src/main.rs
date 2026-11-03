//! `bridle-cli`: the headless command-line program, for scripts, tests and machines without a display. `bridle` runs
//! the same commands and opens the window when given none.

fn main() -> std::process::ExitCode {
    harness_cli::cli::run()
}

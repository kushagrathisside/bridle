//! The real-Chromium test (ADR-0020). It runs the browser self-test against the managed runtime if one is installed, and says
//! so and passes if not, so the ordinary test suite never depends on a download. Use `BRIDLE_HOME` to point at the machine
//! folder that holds `browser-runtime`. This is an automated test on loopback only; it is not the live smoke test, which is
//! `bridle web browser test` run by a person with the network of their choice.

use harness_runtime::http::RemoteRoute;
use harness_runtime::web::browser::runtime::{Layout, inspect};
use harness_runtime::web::browser::selftest;

#[test]
fn the_browser_self_test_passes_when_the_runtime_is_installed() {
    let Some(layout) = Layout::default_location() else {
        eprintln!("skipped: no machine configuration folder (set BRIDLE_HOME)");
        return;
    };
    if !inspect(&layout).ready() {
        eprintln!(
            "skipped: the browser runtime is not installed at {}",
            layout.root.display()
        );
        return;
    }
    let mut lines = Vec::new();
    let checks = selftest::run(
        &layout,
        &RemoteRoute::Direct,
        &|n| std::env::var(n).ok(),
        &mut |l| lines.push(l.to_owned()),
    )
    .expect("the self-test could not run");
    let failed: Vec<String> = checks
        .iter()
        .filter(|c| !c.ok)
        .map(|c| format!("{}: {}", c.name, c.detail))
        .collect();
    assert!(
        failed.is_empty(),
        "{} of {} checks failed:\n{}",
        failed.len(),
        checks.len(),
        failed.join("\n")
    );
    assert!(checks.len() >= 40, "only {} checks ran", checks.len());
}

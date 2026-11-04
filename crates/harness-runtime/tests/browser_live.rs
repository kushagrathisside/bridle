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

/// The child half of the hard-kill test below: open a session, say so, and die without cleaning anything up.
#[test]
fn hard_kill_child() {
    if std::env::var_os("BRIDLE_HARD_KILL_CHILD").is_none() {
        return;
    }
    let layout = Layout::default_location().expect("machine folder");
    let held =
        selftest::hold_one_session(&layout, &RemoteRoute::Direct, &|n| std::env::var(n).ok())
            .expect("session");
    println!("HELD-READY");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    std::mem::forget(held);
    // `abort` ends the process at once: no destructors run, so nothing closes the session. The operating system closes the pipes.
    std::process::abort();
}

#[test]
fn chromium_goes_away_by_itself_when_bridle_is_ended_without_clean_up() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let Some(layout) = Layout::default_location() else {
        eprintln!("skipped: no machine configuration folder");
        return;
    };
    if !inspect(&layout).ready() {
        eprintln!(
            "skipped: the browser runtime is not installed at {}",
            layout.root.display()
        );
        return;
    }
    let mut child = Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "hard_kill_child",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("BRIDLE_HARD_KILL_CHILD", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("child");
    let mut reader = BufReader::new(child.stdout.take().expect("pipe"));
    let started = Instant::now();
    let mut ready = false;
    let mut line = String::new();
    while started.elapsed() < Duration::from_secs(90) {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if line.contains("HELD-READY") {
            ready = true;
            break;
        }
    }
    assert!(ready, "the child never opened a session");
    let running = selftest::processes_under(&layout.browsers_dir());
    assert!(
        running > 0,
        "Chromium was not running while the child held the session"
    );
    let status = child.wait().expect("child ends");
    assert!(!status.success(), "the child was meant to be aborted");
    let ended = Instant::now();
    while selftest::processes_under(&layout.browsers_dir()) > 0
        && ended.elapsed() < Duration::from_secs(30)
    {
        std::thread::sleep(Duration::from_millis(500));
    }
    let left = selftest::processes_under(&layout.browsers_dir());
    assert_eq!(
        left,
        0,
        "{left} Chromium process(es) were still running {} s after the parent was aborted",
        ended.elapsed().as_secs()
    );
    eprintln!(
        "Chromium was gone {:.1} s after the parent was aborted ({running} process(es) before)",
        ended.elapsed().as_secs_f32()
    );
}

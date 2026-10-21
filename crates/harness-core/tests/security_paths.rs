//! Integration tests for the workspace path guard (R-27.1). The check order is documented in
//! `security/path_guard.rs`.

use std::fs;
use std::path::{Path, PathBuf};

use harness_core::security::path_guard::{PathGuard, PathViolation};
use tempfile::TempDir;

/// A temporary directory holding a workspace root called `Project`. Paths are canonical, so they
/// can be compared with the roots the guard stores.
struct Workspace {
    _dir: TempDir,
    parent: PathBuf,
    root: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("create tempdir");
        let parent = dunce::canonicalize(dir.path()).expect("canonicalise tempdir");
        let root = parent.join("Project");
        fs::create_dir(&root).expect("create project root");
        Self {
            _dir: dir,
            parent,
            root,
        }
    }

    fn guard(&self, windows_rules: bool) -> PathGuard {
        PathGuard::new([self.root.clone()], windows_rules).expect("guard over an existing root")
    }
}

#[cfg(unix)]
fn link(target: &Path, at: &Path) {
    std::os::unix::fs::symlink(target, at).expect("create symlink");
}

#[test]
fn dotdot_that_stays_inside_is_allowed() {
    let ws = Workspace::new();
    for windows in [false, true] {
        let got = ws
            .guard(windows)
            .check(Path::new("src/../README.md"), &ws.root);
        assert_eq!(
            got,
            Ok(ws.root.join("README.md")),
            "windows_rules={windows}"
        );
    }
}

#[test]
fn traversal_above_the_filesystem_root_is_rejected() {
    let ws = Workspace::new();
    let deep = "../".repeat(64) + "etc/passwd";
    let deep_backslash = "..\\".repeat(64) + "etc\\passwd";
    for windows in [false, true] {
        let guard = ws.guard(windows);
        assert_eq!(
            guard.check(Path::new(&deep), &ws.root),
            Err(PathViolation::Traversal),
            "windows_rules={windows}"
        );
    }
    assert_eq!(
        ws.guard(true).check(Path::new(&deep_backslash), &ws.root),
        Err(PathViolation::Traversal)
    );
}

#[test]
fn traversal_out_of_the_root_is_outside_the_workspace() {
    let ws = Workspace::new();
    for windows in [false, true] {
        let got = ws
            .guard(windows)
            .check(Path::new("../sibling/secret.txt"), &ws.root);
        assert!(
            matches!(got, Err(PathViolation::OutsideWorkspace { .. })),
            "windows_rules={windows}: {got:?}"
        );
    }
}

#[test]
fn absolute_path_outside_the_root_is_rejected() {
    let ws = Workspace::new();
    for windows in [false, true] {
        let got = ws.guard(windows).check(Path::new("/etc/passwd"), &ws.root);
        assert!(
            matches!(got, Err(PathViolation::OutsideWorkspace { .. })),
            "windows_rules={windows}: {got:?}"
        );
    }
}

#[test]
fn windows_drive_path_is_outside_the_workspace_on_any_host() {
    let ws = Workspace::new();
    for windows in [false, true] {
        let guard = ws.guard(windows);
        for raw in [r"C:\Windows\x", "C:/Windows/x"] {
            let got = guard.check(Path::new(raw), &ws.root);
            assert!(
                matches!(got, Err(PathViolation::OutsideWorkspace { .. })),
                "windows_rules={windows}, {raw}: {got:?}"
            );
        }
    }
}

#[test]
fn new_files_and_directories_inside_the_root_are_allowed() {
    let ws = Workspace::new();
    for windows in [false, true] {
        let guard = ws.guard(windows);
        assert_eq!(
            guard.check(Path::new("src/new_dir/new_file.rs"), &ws.root),
            Ok(ws.root.join("src/new_dir/new_file.rs")),
            "windows_rules={windows}"
        );
        assert_eq!(
            guard.check(Path::new("new.rs"), &ws.root.join("src")),
            Ok(ws.root.join("src/new.rs")),
            "windows_rules={windows}, relative base"
        );
    }
}

#[test]
fn backslash_separators_are_normalised_under_windows_rules() {
    let ws = Workspace::new();
    assert_eq!(
        ws.guard(true).check(Path::new(r"src\main.rs"), &ws.root),
        Ok(ws.root.join("src").join("main.rs"))
    );
}

#[test]
fn reserved_device_names_are_refused_with_windows_rules() {
    let ws = Workspace::new();
    let guard = ws.guard(true);
    for name in [
        "CON",
        "prn",
        "AUX",
        "NUL",
        "COM1",
        "com9",
        "LPT5",
        "aux.c",
        "NUL.tar.gz",
        "src/con",
        "src\\aux.txt",
        "COM\u{b9}",
        "COM\u{b2}",
        "LPT\u{b3}.log",
        "con ",
        "nul.",
    ] {
        let got = guard.check(Path::new(name), &ws.root);
        assert!(
            matches!(got, Err(PathViolation::ReservedDeviceName(_))),
            "{name}: {got:?}"
        );
    }
}

#[test]
fn names_that_only_resemble_device_names_are_allowed_with_windows_rules() {
    let ws = Workspace::new();
    let guard = ws.guard(true);
    for name in ["console.txt", "COM10", "nul_notes.md", "auxiliary/file.txt"] {
        assert_eq!(
            guard.check(Path::new(name), &ws.root),
            Ok(ws.root.join(name)),
            "{name}"
        );
    }
}

#[test]
fn device_names_are_allowed_without_windows_rules() {
    let ws = Workspace::new();
    assert_eq!(
        ws.guard(false).check(Path::new("aux.c"), &ws.root),
        Ok(ws.root.join("aux.c"))
    );
}

#[test]
fn alternate_data_streams_are_refused_with_windows_rules() {
    let ws = Workspace::new();
    let guard = ws.guard(true);
    for name in [
        "a.txt:stream",
        "a.txt::$DATA",
        "src/a.txt:Zone.Identifier",
        "C:foo",
    ] {
        assert_eq!(
            guard.check(Path::new(name), &ws.root),
            Err(PathViolation::AlternateDataStream),
            "{name}"
        );
    }
}

#[test]
fn unc_and_device_namespace_paths_are_refused_with_windows_rules() {
    let ws = Workspace::new();
    let guard = ws.guard(true);
    for raw in [
        r"\\server\share\file.txt",
        "//server/share/file.txt",
        r"\\?\C:\x",
        r"\\.\pipe\x",
    ] {
        assert_eq!(
            guard.check(Path::new(raw), &ws.root),
            Err(PathViolation::UncOrDevicePath),
            "{raw}"
        );
    }
}

#[test]
fn trailing_dot_or_space_in_a_component_is_refused_with_windows_rules() {
    let ws = Workspace::new();
    let guard = ws.guard(true);
    for name in [
        "notes.",
        "notes ",
        "src./main.rs",
        "src /main.rs",
        "dir.../x",
    ] {
        assert_eq!(
            guard.check(Path::new(name), &ws.root),
            Err(PathViolation::TrailingDotOrSpace),
            "{name}"
        );
    }
}

#[test]
fn case_insensitive_containment_applies_only_under_windows_rules() {
    let ws = Workspace::new();
    let upper = format!("{}/PROJECT/a.txt", ws.parent.display());
    assert_eq!(
        ws.guard(true).check(Path::new(&upper), &ws.root),
        Ok(ws.root.join("a.txt")),
        "windows_rules=true should match the root case-insensitively"
    );
    let got = ws.guard(false).check(Path::new(&upper), &ws.root);
    assert!(
        matches!(got, Err(PathViolation::OutsideWorkspace { .. })),
        "windows_rules=false: {got:?}"
    );
}

#[test]
fn any_of_several_roots_contains_a_path() {
    let a = Workspace::new();
    let b = Workspace::new();
    let guard = PathGuard::new([a.root.clone(), b.root.clone()], false).expect("two roots");
    assert_eq!(
        guard.check(Path::new("x.txt"), &b.root),
        Ok(b.root.join("x.txt"))
    );
    assert_eq!(
        guard.check(&a.root.join("y.txt"), &b.root),
        Ok(a.root.join("y.txt"))
    );
    let got = guard.check(Path::new("/etc/passwd"), &a.root);
    assert!(matches!(got, Err(PathViolation::OutsideWorkspace { .. })));
}

#[test]
fn empty_and_nul_candidates_are_refused() {
    let ws = Workspace::new();
    for windows in [false, true] {
        let guard = ws.guard(windows);
        assert_eq!(
            guard.check(Path::new(""), &ws.root),
            Err(PathViolation::Empty)
        );
        assert_eq!(
            guard.check(Path::new("src\0x"), &ws.root),
            Err(PathViolation::InvalidCharacter),
            "windows_rules={windows}"
        );
    }
}

#[test]
fn relative_base_cannot_prove_containment() {
    let ws = Workspace::new();
    let got = ws
        .guard(false)
        .check(Path::new("file.txt"), Path::new("relative/base"));
    assert!(matches!(got, Err(PathViolation::OutsideWorkspace { .. })));
}

#[test]
fn missing_root_is_an_io_error() {
    let ws = Workspace::new();
    assert!(PathGuard::new([ws.root.join("does-not-exist")], false).is_err());
}

#[test]
fn for_platform_uses_the_host_windows_rules_and_canonical_roots() {
    let ws = Workspace::new();
    let guard = PathGuard::for_platform([ws.root.clone()]).expect("guard over an existing root");
    assert_eq!(guard.windows_rules(), cfg!(windows));
    assert_eq!(guard.roots(), std::slice::from_ref(&ws.root));
}

#[cfg(unix)]
#[test]
fn symlink_to_a_directory_outside_the_root_is_a_symlink_escape() {
    let ws = Workspace::new();
    let outside = tempfile::tempdir().expect("outside tempdir");
    let outside = dunce::canonicalize(outside.path()).expect("canonicalise outside");
    fs::write(outside.join("secret.txt"), "secret").expect("write secret");
    link(&outside, &ws.root.join("escape"));

    for windows in [false, true] {
        let guard = ws.guard(windows);
        assert_eq!(
            guard.check(Path::new("escape/secret.txt"), &ws.root),
            Err(PathViolation::SymlinkEscape {
                resolved: outside.join("secret.txt").display().to_string()
            }),
            "existing file through a link, windows_rules={windows}"
        );
        assert!(
            matches!(
                guard.check(Path::new("escape/new.txt"), &ws.root),
                Err(PathViolation::SymlinkEscape { .. })
            ),
            "new file through a link, windows_rules={windows}"
        );
        assert!(
            matches!(
                guard.check(Path::new("escape"), &ws.root),
                Err(PathViolation::SymlinkEscape { .. })
            ),
            "the link itself, windows_rules={windows}"
        );
    }
}

#[cfg(unix)]
#[test]
fn symlink_to_a_file_outside_the_root_is_a_symlink_escape() {
    let ws = Workspace::new();
    let outside = tempfile::tempdir().expect("outside tempdir");
    let secret = dunce::canonicalize(outside.path())
        .expect("canonicalise outside")
        .join("secret.txt");
    fs::write(&secret, "secret").expect("write secret");
    link(&secret, &ws.root.join("secret_link"));

    assert!(matches!(
        ws.guard(false).check(Path::new("secret_link"), &ws.root),
        Err(PathViolation::SymlinkEscape { .. })
    ));
}

#[cfg(unix)]
#[test]
fn symlink_inside_the_root_is_allowed_and_resolved() {
    let ws = Workspace::new();
    fs::create_dir(ws.root.join("real")).expect("create real dir");
    link(&ws.root.join("real"), &ws.root.join("alias"));

    assert_eq!(
        ws.guard(false).check(Path::new("alias/new.txt"), &ws.root),
        Ok(ws.root.join("real").join("new.txt"))
    );
}

#[cfg(unix)]
#[test]
fn dangling_symlinks_are_refused_as_unverifiable() {
    let ws = Workspace::new();
    let outside = tempfile::tempdir().expect("outside tempdir");
    let outside = dunce::canonicalize(outside.path()).expect("canonicalise outside");
    link(&outside.join("not-yet"), &ws.root.join("dangling_out"));
    // Also refused when the target is inside the root: the check does not follow dangling links.
    link(&ws.root.join("later.txt"), &ws.root.join("dangling_in"));

    for name in ["dangling_out", "dangling_in"] {
        assert!(
            matches!(
                ws.guard(false).check(Path::new(name), &ws.root),
                Err(PathViolation::SymlinkEscape { .. })
            ),
            "{name}"
        );
    }
}

#[cfg(unix)]
#[test]
fn dotdot_after_a_symlink_is_resolved_lexically_and_the_result_is_returned() {
    let ws = Workspace::new();
    let outside = tempfile::tempdir().expect("outside tempdir");
    let outside = dunce::canonicalize(outside.path()).expect("canonicalise outside");
    link(&outside, &ws.root.join("escape"));

    // The guard returns `root/inside.txt`, the lexically normalised path. The OS would resolve
    // `escape/..` through the link, so callers must use the returned path, not the candidate.
    assert_eq!(
        ws.guard(false)
            .check(Path::new("escape/../inside.txt"), &ws.root),
        Ok(ws.root.join("inside.txt"))
    );
}

//! A unified diff of two texts, for showing a change to the person (the chat shows what `edit_file` and `write_file` did).
//!
//! The common start and end of the two texts are set aside first, so an edit, which is local, leaves a small middle. The middle is
//! compared line by line with a longest-common-subsequence table when it is small enough (at most four million cells); a larger middle is
//! shown as all removed and then all added. The output has the usual `---`, `+++` and `@@` lines with three lines of context, and is cut
//! at a fixed number of lines with a note, so a rewrite of a large file cannot flood the chat. Nothing here touches a file.

/// Context lines shown around each change.
const CONTEXT: usize = 3;
/// The most lines of diff returned.
const MAX_LINES: usize = 240;
/// The largest comparison table, in cells.
const MAX_CELLS: usize = 4_000_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Same,
    Removed,
    Added,
}

struct Op<'a> {
    kind: Kind,
    text: &'a str,
}

/// The unified diff from `old` to `new` for `path`, or `None` when the texts are the same.
pub fn unified(path: &str, old: &str, new: &str) -> Option<String> {
    if old == new {
        return None;
    }
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let mid_a = &a[prefix..a.len() - suffix];
    let mid_b = &b[prefix..b.len() - suffix];

    let mut ops: Vec<Op> = Vec::with_capacity(a.len() + mid_b.len());
    ops.extend(a[..prefix].iter().map(|t| Op {
        kind: Kind::Same,
        text: t,
    }));
    ops.extend(compare(mid_a, mid_b));
    ops.extend(a[a.len() - suffix..].iter().map(|t| Op {
        kind: Kind::Same,
        text: t,
    }));
    // Texts that differ only in a final line break have equal lines: say so instead of showing nothing.
    if ops.iter().all(|op| op.kind == Kind::Same) {
        return Some(format!(
            "--- a/{path}\n+++ b/{path}\n(only the end of the last line differs)\n"
        ));
    }

    // Each operation's line number in the old and the new text (1-based, the number the next line would have).
    let mut positions = Vec::with_capacity(ops.len());
    let (mut line_a, mut line_b) = (1_usize, 1_usize);
    for op in &ops {
        positions.push((line_a, line_b));
        match op.kind {
            Kind::Same => {
                line_a += 1;
                line_b += 1;
            }
            Kind::Removed => line_a += 1,
            Kind::Added => line_b += 1,
        }
    }

    // The ranges of operations to show: each change with its context, merged where they touch.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        if op.kind == Kind::Same {
            continue;
        }
        let from = i.saturating_sub(CONTEXT);
        let to = (i + CONTEXT + 1).min(ops.len());
        match ranges.last_mut() {
            Some(last) if from <= last.1 => last.1 = to.max(last.1),
            _ => ranges.push((from, to)),
        }
    }

    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    let mut shown = 0_usize;
    let mut cut = 0_usize;
    for (from, to) in ranges {
        let slice = &ops[from..to];
        let old_len = slice.iter().filter(|o| o.kind != Kind::Added).count();
        let new_len = slice.iter().filter(|o| o.kind != Kind::Removed).count();
        let (start_a, start_b) = positions[from];
        let header = format!(
            "@@ -{},{} +{},{} @@\n",
            if old_len == 0 {
                start_a.saturating_sub(1)
            } else {
                start_a
            },
            old_len,
            if new_len == 0 {
                start_b.saturating_sub(1)
            } else {
                start_b
            },
            new_len
        );
        let room = MAX_LINES.saturating_sub(shown + 1);
        if room == 0 {
            cut += slice.len() + 1;
            continue;
        }
        out.push_str(&header);
        shown += 1;
        cut += slice.len().saturating_sub(room);
        for op in slice.iter().take(room) {
            out.push(match op.kind {
                Kind::Same => ' ',
                Kind::Removed => '-',
                Kind::Added => '+',
            });
            out.push_str(op.text);
            out.push('\n');
            shown += 1;
        }
    }
    if cut > 0 {
        out.push_str(&format!(
            "... {cut} more line(s) of the change are not shown\n"
        ));
    }
    Some(out)
}

/// The operations that turn `a` into `b`.
fn compare<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Op<'a>> {
    if a.is_empty() || b.is_empty() || a.len().saturating_mul(b.len()) > MAX_CELLS {
        let mut ops: Vec<Op> = a
            .iter()
            .map(|t| Op {
                kind: Kind::Removed,
                text: t,
            })
            .collect();
        ops.extend(b.iter().map(|t| Op {
            kind: Kind::Added,
            text: t,
        }));
        return ops;
    }
    let (n, m) = (a.len(), b.len());
    // lcs[i][j]: the length of the longest common subsequence of a[i..] and b[j..].
    let width = m + 1;
    let mut lcs = vec![0_u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * width + j] = if a[i] == b[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let mut ops = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push(Op {
                kind: Kind::Same,
                text: a[i],
            });
            i += 1;
            j += 1;
        } else if lcs[(i + 1) * width + j] >= lcs[i * width + j + 1] {
            ops.push(Op {
                kind: Kind::Removed,
                text: a[i],
            });
            i += 1;
        } else {
            ops.push(Op {
                kind: Kind::Added,
                text: b[j],
            });
            j += 1;
        }
    }
    ops.extend(a[i..].iter().map(|t| Op {
        kind: Kind::Removed,
        text: t,
    }));
    ops.extend(b[j..].iter().map(|t| Op {
        kind: Kind::Added,
        text: t,
    }));
    ops
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_same_text_has_no_diff_and_a_changed_line_shows_with_context() {
        assert_eq!(unified("a.txt", "x\ny\n", "x\ny\n"), None);
        let old = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n";
        let new = "one\ntwo\nthree\nfour\nFIVE\nsix\nseven\neight\nnine\n";
        let d = unified("a.txt", old, new).unwrap();
        assert_eq!(
            d,
            "--- a/a.txt\n+++ b/a.txt\n@@ -2,7 +2,7 @@\n two\n three\n four\n-five\n+FIVE\n six\n seven\n eight\n"
        );
    }

    #[test]
    fn two_distant_changes_make_two_hunks_and_near_ones_merge() {
        let old: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        let far = old
            .replace("line 3\n", "THREE\n")
            .replace("line 28\n", "TWENTY-EIGHT\n");
        let d = unified("f", &old, &far).unwrap();
        assert_eq!(d.matches("@@ -").count(), 2, "{d}");
        let near = old
            .replace("line 10\n", "TEN\n")
            .replace("line 13\n", "THIRTEEN\n");
        assert_eq!(
            unified("f", &old, &near).unwrap().matches("@@ -").count(),
            1
        );
    }

    #[test]
    fn an_added_a_removed_and_a_new_file_are_shown() {
        let d = unified("n.txt", "", "a\nb\n").unwrap();
        assert!(d.contains("@@ -0,0 +1,2 @@\n+a\n+b\n"), "{d}");
        let d = unified("n.txt", "a\nb\nc\n", "a\nc\n").unwrap();
        assert!(d.contains("-b\n") && !d.contains("+b"), "{d}");
        let d = unified("n.txt", "a\nb\n", "a\nb\nc\n").unwrap();
        assert!(d.contains("+c\n"), "{d}");
    }

    #[test]
    fn a_large_rewrite_is_cut_with_a_note_and_a_huge_middle_does_not_blow_up() {
        let old: String = (0..2000).map(|n| format!("old {n}\n")).collect();
        let new: String = (0..2000).map(|n| format!("new {n}\n")).collect();
        let d = unified("big", &old, &new).unwrap();
        assert!(d.lines().count() <= MAX_LINES + 6, "{}", d.lines().count());
        assert!(
            d.contains("more line(s) of the change are not shown"),
            "{}",
            &d[d.len() - 80..]
        );
        // Beyond the table limit the middle is shown as removed then added, and it still returns quickly.
        let a: String = (0..3000).map(|n| format!("a {n}\n")).collect();
        let b: String = (0..3000).map(|n| format!("b {n}\n")).collect();
        assert!(unified("huge", &a, &b).is_some());
    }

    #[test]
    fn a_difference_only_in_the_final_line_break_is_still_reported() {
        let d = unified("e", "a\nb", "a\nb\n").unwrap();
        assert!(d.contains("only the end of the last line differs"), "{d}");
    }
}

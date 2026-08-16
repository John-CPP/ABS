//! Line-based unified diffs for PKGBUILD preview.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tag {
    Equal,
    Delete,
    Insert,
}

/// Kind of a unified-diff line, for preview coloring.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffLineKind {
    HeaderOld,
    HeaderNew,
    Hunk,
    Delete,
    Insert,
    Context,
}

/// `---` / `+++` before `-` / `+` so file headers are not treated as edits.
pub fn classify_diff_line(line: &str) -> DiffLineKind {
    if line.starts_with("---") {
        DiffLineKind::HeaderOld
    } else if line.starts_with("+++") {
        DiffLineKind::HeaderNew
    } else if line.starts_with("@@") {
        DiffLineKind::Hunk
    } else if line.starts_with('-') {
        DiffLineKind::Delete
    } else if line.starts_with('+') {
        DiffLineKind::Insert
    } else {
        DiffLineKind::Context
    }
}

/// Unified diff of `old` → `new`. Empty string means the texts are identical.
pub fn unified_diff(old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a == b {
        // Differ only in a trailing newline.
        return format!(
            "--- last PKGBUILD\n+++ AUR PKGBUILD\n@@ -{n},0 +{n},0 @@\n",
            n = a.len()
        );
    }
    let ops = diff_ops(&a, &b);
    emit_unified(&a, &b, &ops)
}

fn diff_ops(a: &[&str], b: &[&str]) -> Vec<Tag> {
    let n = a.len();
    let m = b.len();
    if n.saturating_mul(m) > 1_500_000 {
        return greedy_ops(a, b);
    }
    let w = m + 1;
    let mut dp = vec![0u32; (n + 1) * w];
    for i in 0..n {
        for j in 0..m {
            let idx = (i + 1) * w + (j + 1);
            dp[idx] = if a[i] == b[j] {
                dp[i * w + j] + 1
            } else {
                dp[(i + 1) * w + j].max(dp[i * w + (j + 1)])
            };
        }
    }
    let mut ops = Vec::new();
    let mut i = n;
    let mut j = m;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && a[i - 1] == b[j - 1] {
            ops.push(Tag::Equal);
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i * w + (j - 1)] >= dp[(i - 1) * w + j]) {
            ops.push(Tag::Insert);
            j -= 1;
        } else {
            ops.push(Tag::Delete);
            i -= 1;
        }
    }
    ops.reverse();
    ops
}

fn greedy_ops(a: &[&str], b: &[&str]) -> Vec<Tag> {
    let mut ops = Vec::new();
    let mut i = 0;
    let mut j = 0;
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            ops.push(Tag::Equal);
            i += 1;
            j += 1;
        } else if j < b.len() && (i == a.len() || !b[j..].contains(&a[i])) {
            ops.push(Tag::Insert);
            j += 1;
        } else {
            ops.push(Tag::Delete);
            i += 1;
        }
    }
    ops
}

fn emit_unified(a: &[&str], b: &[&str], ops: &[Tag]) -> String {
    const CTX: usize = 3;
    let mut hunks: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut ai = 0usize;
    let mut bi = 0usize;
    let mut k = 0usize;
    while k < ops.len() {
        if ops[k] == Tag::Equal {
            ai += 1;
            bi += 1;
            k += 1;
            continue;
        }
        let a_change_start = ai;
        let b_change_start = bi;
        while k < ops.len() && ops[k] != Tag::Equal {
            match ops[k] {
                Tag::Delete => ai += 1,
                Tag::Insert => bi += 1,
                Tag::Equal => {}
            }
            k += 1;
        }
        let a0 = a_change_start.saturating_sub(CTX);
        let b0 = b_change_start.saturating_sub(CTX);
        let a1 = (ai + CTX).min(a.len());
        let b1 = (bi + CTX).min(b.len());
        if let Some(last) = hunks.last_mut() {
            if a0 <= last.1 {
                last.1 = a1;
                last.3 = b1;
                continue;
            }
        }
        hunks.push((a0, a1, b0, b1));
    }

    let mut out = String::from("--- last PKGBUILD\n+++ AUR PKGBUILD\n");
    for (a0, a1, b0, b1) in hunks {
        let a_count = a1 - a0;
        let b_count = b1 - b0;
        let a_start = if a_count == 0 { a0 } else { a0 + 1 };
        let b_start = if b_count == 0 { b0 } else { b0 + 1 };
        out.push_str(&format!(
            "@@ -{a_start},{a_count} +{b_start},{b_count} @@\n"
        ));
        let mut i = 0usize;
        let mut j = 0usize;
        for tag in ops {
            match tag {
                Tag::Equal => {
                    if i >= a0 && i < a1 {
                        out.push(' ');
                        out.push_str(a[i]);
                        out.push('\n');
                    }
                    i += 1;
                    j += 1;
                }
                Tag::Delete => {
                    if i >= a0 && i < a1 {
                        out.push('-');
                        out.push_str(a[i]);
                        out.push('\n');
                    }
                    i += 1;
                }
                Tag::Insert => {
                    if j >= b0 && j < b1 {
                        out.push('+');
                        out.push_str(b[j]);
                        out.push('\n');
                    }
                    j += 1;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_is_empty() {
        assert!(unified_diff("a\nb\n", "a\nb\n").is_empty());
    }

    #[test]
    fn replaces_a_line() {
        let diff = unified_diff("a\nb\nc\n", "a\nx\nc\n");
        assert!(diff.contains("--- last PKGBUILD"), "{diff}");
        assert!(diff.contains("+++ AUR PKGBUILD"), "{diff}");
        assert!(diff.contains("-b"), "{diff}");
        assert!(diff.contains("+x"), "{diff}");
        assert!(diff.contains(" a"), "{diff}");
        assert!(diff.contains(" c"), "{diff}");
    }

    #[test]
    fn inserts_at_end() {
        let diff = unified_diff("a\n", "a\nb\n");
        assert!(diff.contains("+b"), "{diff}");
        assert!(diff.contains(" a"), "{diff}");
    }

    #[test]
    fn classifies_headers_apart_from_edits() {
        assert_eq!(
            classify_diff_line("--- last PKGBUILD"),
            DiffLineKind::HeaderOld
        );
        assert_eq!(
            classify_diff_line("+++ AUR PKGBUILD"),
            DiffLineKind::HeaderNew
        );
        assert_eq!(classify_diff_line("@@ -1,2 +1,2 @@"), DiffLineKind::Hunk);
        assert_eq!(classify_diff_line("-pkgver=1.0"), DiffLineKind::Delete);
        assert_eq!(classify_diff_line("+pkgver=2.0"), DiffLineKind::Insert);
        assert_eq!(classify_diff_line(" pkgrel=1"), DiffLineKind::Context);
    }
}

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// Function context git prints after the `@@` range, if any.
    pub section: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
    pub hunks: Vec<Hunk>,
    /// The patch wasn't available (GitHub omits it for very large files);
    /// the local diff will fill it in.
    pub patch_missing: bool,
}

impl FileDiff {
    fn new(path: String) -> Self {
        Self {
            path,
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            additions: 0,
            deletions: 0,
            hunks: Vec::new(),
            patch_missing: false,
        }
    }
}

/// Parses `git diff` output (with `a/` and `b/` prefixes).
pub fn parse_git_diff(text: &str) -> Vec<FileDiff> {
    let mut files = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(header) = line.strip_prefix("diff --git ") else { continue };
        let mut file = FileDiff::new(path_from_diff_header(header));
        let mut old_path = None;

        while let Some(&l) = lines.peek() {
            if l.starts_with("diff --git ") || l.starts_with("@@ ") {
                break;
            }
            lines.next();
            if l.starts_with("new file mode") {
                file.status = FileStatus::Added;
            } else if l.starts_with("deleted file mode") {
                file.status = FileStatus::Deleted;
            } else if let Some(p) = l.strip_prefix("rename from ") {
                file.status = FileStatus::Renamed;
                old_path = Some(unquote(p));
            } else if let Some(p) = l.strip_prefix("rename to ") {
                file.path = unquote(p);
            } else if let Some(p) = l.strip_prefix("copy from ") {
                file.status = FileStatus::Copied;
                old_path = Some(unquote(p));
            } else if let Some(p) = l.strip_prefix("copy to ") {
                file.path = unquote(p);
            } else if l.starts_with("Binary files ") || l == "GIT binary patch" {
                file.binary = true;
            } else if let Some(p) = l.strip_prefix("+++ ") {
                if let Some(p) = strip_prefix_path(p, "b/") {
                    file.path = p;
                }
            } else if let Some(p) = l.strip_prefix("--- ") {
                if let Some(p) = strip_prefix_path(p, "a/") {
                    if file.status == FileStatus::Modified && p != file.path {
                        old_path = Some(p);
                    }
                }
            }
        }

        file.old_path = old_path;
        file.hunks = parse_hunks(&mut lines);
        count(&mut file);
        files.push(file);
    }
    files
}

/// Parses a bare patch (hunks only), as GitHub returns per file.
pub fn parse_patch(patch: &str) -> Vec<Hunk> {
    parse_hunks(&mut patch.lines().peekable())
}

fn parse_hunks<'a, I: Iterator<Item = &'a str>>(lines: &mut std::iter::Peekable<I>) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    while let Some(&l) = lines.peek() {
        if l.starts_with("diff --git ") {
            break;
        }
        lines.next();
        let Some(mut hunk) = parse_hunk_header(l) else { continue };
        let (mut old, mut new) = (hunk.old_start, hunk.new_start);
        let (mut old_left, mut new_left) = (hunk.old_lines, hunk.new_lines);

        while old_left > 0 || new_left > 0 {
            let Some(&l) = lines.peek() else { break };
            let (kind, text) = match l.as_bytes().first() {
                Some(b'+') => (LineKind::Add, &l[1..]),
                Some(b'-') => (LineKind::Del, &l[1..]),
                Some(b' ') => (LineKind::Context, &l[1..]),
                // Some tools strip the trailing space of empty context lines.
                None => (LineKind::Context, ""),
                Some(b'\\') => {
                    lines.next();
                    continue;
                }
                _ => break,
            };
            lines.next();
            let line = match kind {
                LineKind::Add => {
                    new_left = new_left.saturating_sub(1);
                    new += 1;
                    DiffLine { kind, old: None, new: Some(new - 1), text: text.into() }
                }
                LineKind::Del => {
                    old_left = old_left.saturating_sub(1);
                    old += 1;
                    DiffLine { kind, old: Some(old - 1), new: None, text: text.into() }
                }
                LineKind::Context => {
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                    old += 1;
                    new += 1;
                    DiffLine { kind, old: Some(old - 1), new: Some(new - 1), text: text.into() }
                }
            };
            hunk.lines.push(line);
        }
        if lines.peek().is_some_and(|l| l.starts_with('\\')) {
            lines.next();
        }
        hunks.push(hunk);
    }
    hunks
}

/// `@@ -a[,b] +c[,d] @@ section`
fn parse_hunk_header(l: &str) -> Option<Hunk> {
    let rest = l.strip_prefix("@@ -")?;
    let (ranges, section) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |r: &str| -> Option<(u32, u32)> {
        match r.split_once(',') {
            Some((s, n)) => Some((s.parse().ok()?, n.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (old_start, old_lines) = range(old)?;
    let (new_start, new_lines) = range(new)?;
    Some(Hunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        section: section.trim_start().to_owned(),
        lines: Vec::new(),
    })
}

fn count(file: &mut FileDiff) {
    for line in file.hunks.iter().flat_map(|h| &h.lines) {
        match line.kind {
            LineKind::Add => file.additions += 1,
            LineKind::Del => file.deletions += 1,
            LineKind::Context => {}
        }
    }
}

fn strip_prefix_path(p: &str, prefix: &str) -> Option<String> {
    let p = unquote(p);
    p.strip_prefix(prefix).map(str::to_owned)
}

/// `a/x b/x` — ambiguous when paths contain spaces; take the symmetric split.
/// Only used when no `---`/`+++`/rename lines name the file (binary, mode-only).
fn path_from_diff_header(header: &str) -> String {
    if header.starts_with('"') {
        if let Some(end) = header[1..].find("\" ") {
            if let Some(b) = strip_prefix_path(&header[end + 3..], "b/") {
                return b;
            }
        }
    }
    let len = header.len();
    if len % 2 == 1 {
        let mid = len / 2;
        if &header[mid..mid + 1] == " " {
            if let Some(b) = header[mid + 1..].strip_prefix("b/") {
                return b.to_owned();
            }
        }
    }
    header.split_once(" b/").map_or(header, |(_, b)| b).to_owned()
}

/// Undoes git's C-style quoting of unusual paths.
fn unquote(p: &str) -> String {
    let Some(inner) = p.strip_prefix('"').and_then(|p| p.strip_suffix('"')) else {
        return p.to_owned();
    };
    let mut out = Vec::with_capacity(inner.len());
    let mut bytes = inner.bytes();
    while let Some(b) = bytes.next() {
        if b != b'\\' {
            out.push(b);
            continue;
        }
        match bytes.next() {
            Some(b'n') => out.push(b'\n'),
            Some(b't') => out.push(b'\t'),
            Some(b'"') => out.push(b'"'),
            Some(b'\\') => out.push(b'\\'),
            Some(d @ b'0'..=b'7') => {
                let mut v = (d - b'0') as u32;
                for _ in 0..2 {
                    if let Some(d) = bytes.next() {
                        v = v * 8 + (d - b'0') as u32;
                    }
                }
                out.push(v as u8);
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,5 @@ mod a;
 use std::fmt;
-fn old() {}
+fn new() {}
+fn extra() {}
 
 fn tail() {}
@@ -10 +11 @@
-x
\\ No newline at end of file
+y
\\ No newline at end of file
diff --git a/old name.txt b/new name.txt
similarity index 90%
rename from old name.txt
rename to new name.txt
index 3333333..4444444 100644
--- a/old name.txt
+++ b/new name.txt
@@ -1 +1 @@
-a
+b
diff --git a/img.png b/img.png
new file mode 100644
index 0000000..5555555
Binary files /dev/null and b/img.png differ
diff --git a/gone.rs b/gone.rs
deleted file mode 100644
index 6666666..0000000
--- a/gone.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-one
-two
diff --git \"a/tab\\there.txt\" \"b/tab\\there.txt\"
new file mode 100644
--- /dev/null
+++ \"b/tab\\there.txt\"
@@ -0,0 +1 @@
+hi
";

    #[test]
    fn parses_files() {
        let files = parse_git_diff(SAMPLE);
        let summary: Vec<_> = files.iter().map(|f| (f.path.as_str(), f.status, f.additions, f.deletions)).collect();
        assert_eq!(
            summary,
            vec![
                ("src/lib.rs", FileStatus::Modified, 3, 2),
                ("new name.txt", FileStatus::Renamed, 1, 1),
                ("img.png", FileStatus::Added, 0, 0),
                ("gone.rs", FileStatus::Deleted, 0, 2),
                ("tab\there.txt", FileStatus::Added, 1, 0),
            ]
        );
        assert_eq!(files[1].old_path.as_deref(), Some("old name.txt"));
        assert!(files[2].binary);
    }

    #[test]
    fn line_numbers() {
        let files = parse_git_diff(SAMPLE);
        let h = &files[0].hunks[0];
        assert_eq!(h.section, "mod a;");
        let nums: Vec<_> = h.lines.iter().map(|l| (l.kind, l.old, l.new)).collect();
        assert_eq!(
            nums,
            vec![
                (LineKind::Context, Some(1), Some(1)),
                (LineKind::Del, Some(2), None),
                (LineKind::Add, None, Some(2)),
                (LineKind::Add, None, Some(3)),
                (LineKind::Context, Some(3), Some(4)),
                (LineKind::Context, Some(4), Some(5)),
            ]
        );
        assert_eq!(h.lines[4].text, "");
        let h2 = &files[0].hunks[1];
        assert_eq!((h2.old_start, h2.old_lines, h2.new_start, h2.new_lines), (10, 1, 11, 1));
        assert_eq!(h2.lines.len(), 2);
    }

    #[test]
    fn bare_patch_matches_git_hunks() {
        let patch = "@@ -1,4 +1,5 @@ mod a;\n use std::fmt;\n-fn old() {}\n+fn new() {}\n+fn extra() {}\n \n fn tail() {}\n@@ -10 +11 @@\n-x\n\\ No newline at end of file\n+y\n\\ No newline at end of file";
        assert_eq!(parse_patch(patch), parse_git_diff(SAMPLE)[0].hunks);
    }

    #[test]
    fn header_path_with_spaces_and_no_markers() {
        assert_eq!(path_from_diff_header("a/my file.bin b/my file.bin"), "my file.bin");
    }
}

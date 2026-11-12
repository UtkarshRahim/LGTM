//! Parsed PR diffs kept in memory so the UI can load the file list first and
//! fetch hunks only for files it is about to show.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use prism_repo::diff::{FileDiff, FileStatus, Hunk, LineKind};
use prism_repo::order::{self, FileKind};
use prism_syntax::LineSpans;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DiffSource {
    /// GitHub's per-file patches; call `pr_sync_local` for the local version.
    Api,
    Local,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct FileMeta {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
    pub patch_missing: bool,
    pub kind: FileKind,
    /// Identifies the file's change; a viewed mark only holds while it matches.
    pub patch_hash: String,
    /// Rows the file body takes in each layout (hunk headers included), so
    /// the UI can reserve space before the hunks are loaded.
    pub unified_rows: u32,
    pub split_rows: u32,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct FileHunks {
    pub path: String,
    pub hunks: Vec<Hunk>,
    /// One entry per line across all hunks, in order; `None` when the
    /// language isn't supported or the file is too large.
    pub highlights: Option<Vec<LineSpans>>,
}

pub struct PrDiff {
    files: Vec<FileDiff>,
    kinds: Vec<FileKind>,
    index: HashMap<String, usize>,
    highlights: Vec<OnceLock<Option<Vec<LineSpans>>>>,
    /// Head commit, when this diff came from the local mirror.
    local_head: Option<String>,
}

impl PrDiff {
    pub fn new(files: Vec<FileDiff>) -> Self {
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        let order = order::tree_order(&paths);
        let mut slots: Vec<Option<FileDiff>> = files.into_iter().map(Some).collect();
        let (files, kinds): (Vec<FileDiff>, Vec<FileKind>) =
            order.into_iter().map(|(i, k)| (slots[i].take().expect("index used once"), k)).unzip();
        let index = files.iter().enumerate().map(|(i, f)| (f.path.clone(), i)).collect();
        let highlights = files.iter().map(|_| OnceLock::new()).collect();
        Self { files, kinds, index, highlights, local_head: None }
    }

    pub fn local(mut self, head: &str) -> Self {
        self.local_head = Some(head.to_owned());
        self
    }

    pub fn is_local_at(&self, head: &str) -> bool {
        self.local_head.as_deref() == Some(head)
    }

    pub fn local_head(&self) -> Option<&str> {
        self.local_head.as_deref()
    }

    pub fn metas(&self) -> Vec<FileMeta> {
        self.files
            .iter()
            .zip(&self.kinds)
            .map(|(f, &kind)| FileMeta {
                path: f.path.clone(),
                old_path: f.old_path.clone(),
                status: f.status,
                binary: f.binary,
                additions: f.additions,
                deletions: f.deletions,
                patch_missing: f.patch_missing,
                kind,
                patch_hash: f.patch_hash(),
                unified_rows: f.hunks.iter().map(|h| 1 + h.lines.len() as u32).sum(),
                split_rows: f.hunks.iter().map(split_rows).sum(),
            })
            .collect()
    }

    pub fn hunks(&self, paths: &[String]) -> Vec<FileHunks> {
        paths
            .iter()
            .filter_map(|p| {
                let i = *self.index.get(p)?;
                let f = &self.files[i];
                let highlights = self.highlights[i].get_or_init(|| highlight(f)).clone();
                Some(FileHunks { path: f.path.clone(), hunks: f.hunks.clone(), highlights })
            })
            .collect()
    }
}

/// Header row plus side-by-side rows: a run of deletions followed by a run
/// of additions shares rows; the UI must pair lines the same way.
fn split_rows(h: &Hunk) -> u32 {
    let (mut rows, mut dels, mut adds) = (1u32, 0u32, 0u32);
    for l in &h.lines {
        match l.kind {
            LineKind::Del => {
                if adds > 0 {
                    rows += dels.max(adds);
                    (dels, adds) = (0, 0);
                }
                dels += 1;
            }
            LineKind::Add => adds += 1,
            LineKind::Context => {
                rows += dels.max(adds) + 1;
                (dels, adds) = (0, 0);
            }
        }
    }
    rows + dels.max(adds)
}

/// Highlights each side of the diff separately and maps spans back to lines.
fn highlight(f: &FileDiff) -> Option<Vec<LineSpans>> {
    let lang = prism_syntax::language_for(&f.path)?;
    let lines = f.hunks.iter().flat_map(|h| &h.lines);
    let mut old = Vec::new();
    let mut new = Vec::new();
    // (side, index): side false = old.
    let mut map = Vec::new();
    for l in lines {
        match l.kind {
            LineKind::Del => {
                map.push((false, old.len()));
                old.push(l.text.as_str());
            }
            LineKind::Add => {
                map.push((true, new.len()));
                new.push(l.text.as_str());
            }
            LineKind::Context => {
                map.push((true, new.len()));
                new.push(l.text.as_str());
                old.push(l.text.as_str());
            }
        }
    }
    let old_hl = if old.is_empty() { Vec::new() } else { prism_syntax::highlight_lines(lang, &old)? };
    let new_hl = if new.is_empty() { Vec::new() } else { prism_syntax::highlight_lines(lang, &new)? };
    Some(map.into_iter().map(|(side, i)| if side { new_hl[i].clone() } else { old_hl[i].clone() }).collect())
}

const MAX_CACHED: usize = 12;

#[derive(Default)]
pub struct DiffCache {
    inner: Mutex<(u64, HashMap<String, (u64, Arc<PrDiff>)>)>,
}

impl DiffCache {
    pub fn get(&self, key: &str) -> Option<Arc<PrDiff>> {
        let mut g = self.inner.lock().unwrap();
        g.0 += 1;
        let tick = g.0;
        let entry = g.1.get_mut(key)?;
        entry.0 = tick;
        Some(entry.1.clone())
    }

    pub fn insert(&self, key: String, diff: PrDiff) -> Arc<PrDiff> {
        let diff = Arc::new(diff);
        let mut g = self.inner.lock().unwrap();
        g.0 += 1;
        let tick = g.0;
        g.1.insert(key, (tick, diff.clone()));
        if g.1.len() > MAX_CACHED {
            if let Some(oldest) = g.1.iter().min_by_key(|(_, (t, _))| *t).map(|(k, _)| k.clone()) {
                g.1.remove(&oldest);
            }
        }
        diff
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_repo::diff::parse_git_diff;

    const DIFF: &str = "\
diff --git a/src/a.test.ts b/src/a.test.ts
--- a/src/a.test.ts
+++ b/src/a.test.ts
@@ -1 +1 @@
-old()
+test()
diff --git a/src/a.ts b/src/a.ts
--- a/src/a.ts
+++ b/src/a.ts
@@ -1,3 +1,4 @@
 const x = 1;
-let y = 2;
+let y = 3;
+let z = \"s\";
 done();
diff --git a/yarn.lock b/yarn.lock
--- a/yarn.lock
+++ b/yarn.lock
@@ -1 +1 @@
-a
+b
";

    #[test]
    fn orders_counts_and_highlights() {
        let d = PrDiff::new(parse_git_diff(DIFF));
        let metas = d.metas();
        let order: Vec<_> = metas.iter().map(|m| (m.path.as_str(), m.kind)).collect();
        assert_eq!(
            order,
            [("src/a.test.ts", FileKind::Test), ("src/a.ts", FileKind::Source), ("yarn.lock", FileKind::Generated)]
        );
        // header + 5 lines; split: header, ctx, (del|add), (·|add), ctx
        assert_eq!((metas[1].unified_rows, metas[1].split_rows), (6, 5));

        let h = d.hunks(&["src/a.ts".into(), "missing".into()]);
        assert_eq!(h.len(), 1);
        let hl = h[0].highlights.as_ref().unwrap();
        assert_eq!(hl.len(), 5);
        assert!(!hl[3].is_empty(), "string on the added line is highlighted");
    }

    #[test]
    fn split_rows_pairs_runs() {
        let f = &parse_git_diff("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,4 +1,3 @@\n-a\n-b\n-c\n+A\n x\n+B\n")[0];
        // header, max(3 dels, 1 add) = 3, ctx 1, trailing add 1
        assert_eq!(split_rows(&f.hunks[0]), 6);
    }

    #[test]
    fn cache_evicts_least_recent() {
        let c = DiffCache::default();
        for i in 0..=MAX_CACHED {
            c.insert(i.to_string(), PrDiff::new(vec![]));
            c.get("0");
        }
        assert!(c.get("0").is_some());
        assert!(c.get("1").is_none());
    }
}

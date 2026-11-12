//! File order and classification. Files are listed as a directory tree shows
//! them: at each level, directories before files, each sorted by name.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Source,
    Test,
    /// Generated, lock or vendored; collapsed by default.
    Generated,
}

const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "go.sum",
    "poetry.lock",
    "Pipfile.lock",
    "uv.lock",
    "Gemfile.lock",
    "composer.lock",
    "flake.lock",
    "mix.lock",
    "Podfile.lock",
    "Package.resolved",
    "packages.lock.json",
];

const GENERATED_SUFFIXES: &[&str] = &[
    ".min.js", ".min.css", ".map", ".pb.go", "_pb2.py", "_pb2.pyi", ".pb.swift", ".g.dart", ".freezed.dart", ".snap",
];

const GENERATED_DIRS: &[&str] = &["vendor", "node_modules", "dist", "__generated__", "generated", "third_party", ".yarn"];

const TEST_DIRS: &[&str] = &["test", "tests", "__tests__", "spec", "specs", "testdata", "__mocks__"];

pub fn classify(path: &str) -> FileKind {
    let (dirs, name) = split(path);
    if LOCKFILES.contains(&name)
        || GENERATED_SUFFIXES.iter().any(|s| name.ends_with(s))
        || name.contains(".generated.")
        || dirs.iter().any(|d| GENERATED_DIRS.contains(d))
    {
        return FileKind::Generated;
    }
    if test_stem(name).is_some() || dirs.iter().any(|d| TEST_DIRS.contains(d)) {
        return FileKind::Test;
    }
    FileKind::Source
}

fn split(path: &str) -> (Vec<&str>, &str) {
    let mut parts: Vec<&str> = path.split('/').collect();
    let name = parts.pop().unwrap_or(path);
    (parts, name)
}

/// `foo.rs` -> `foo`, `foo.d.ts` -> `foo`.
fn stem(name: &str) -> &str {
    name.split('.').next().filter(|s| !s.is_empty()).unwrap_or(name)
}

/// The source stem a test file name refers to, if it follows a test naming convention.
fn test_stem(name: &str) -> Option<&str> {
    let base = stem(name);
    if name.contains(".test.") || name.contains(".spec.") {
        return Some(base);
    }
    for suffix in ["_test", "_spec", "_tests", "Test", "Tests", "Spec"] {
        if let Some(s) = base.strip_suffix(suffix).filter(|s| !s.is_empty()) {
            return Some(s);
        }
    }
    base.strip_prefix("test_").filter(|s| !s.is_empty())
}

use std::cmp::Ordering;

/// Compares paths in tree order: directories sort before files at the same level.
pub fn tree_cmp(a: &str, b: &str) -> Ordering {
    let (pa, pb): (Vec<&str>, Vec<&str>) = (a.split('/').collect(), b.split('/').collect());
    for i in 0..pa.len().min(pb.len()) {
        if pa[i] == pb[i] {
            continue;
        }
        let (a_dir, b_dir) = (i + 1 < pa.len(), i + 1 < pb.len());
        return b_dir.cmp(&a_dir).then_with(|| pa[i].cmp(pb[i]));
    }
    pa.len().cmp(&pb.len())
}

/// Indices into `paths` in tree order, with each file's kind.
pub fn tree_order(paths: &[&str]) -> Vec<(usize, FileKind)> {
    let mut idx: Vec<usize> = (0..paths.len()).collect();
    idx.sort_by(|&a, &b| tree_cmp(paths[a], paths[b]));
    idx.into_iter().map(|i| (i, classify(paths[i]))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies() {
        assert_eq!(classify("Cargo.lock"), FileKind::Generated);
        assert_eq!(classify("web/pnpm-lock.yaml"), FileKind::Generated);
        assert_eq!(classify("vendor/github.com/x/y.go"), FileKind::Generated);
        assert_eq!(classify("app/static/app.min.js"), FileKind::Generated);
        assert_eq!(classify("api/user.pb.go"), FileKind::Generated);
        assert_eq!(classify("src/__generated__/schema.ts"), FileKind::Generated);
        assert_eq!(classify("src/user.test.ts"), FileKind::Test);
        assert_eq!(classify("pkg/user_test.go"), FileKind::Test);
        assert_eq!(classify("tests/test_user.py"), FileKind::Test);
        assert_eq!(classify("src/test/java/UserTest.java"), FileKind::Test);
        assert_eq!(classify("src/user.ts"), FileKind::Source);
        assert_eq!(classify("contest.rs"), FileKind::Source);
        assert_eq!(classify("latest.md"), FileKind::Source);
    }

    #[test]
    fn directories_before_files() {
        let paths = ["README.md", "src/ui/user.ts", "src/auth.go", "Cargo.lock", "src/api/user.ts", "tests/a.rs", "src/b.rs"];
        let order: Vec<&str> = tree_order(&paths).into_iter().map(|(i, _)| paths[i]).collect();
        assert_eq!(
            order,
            ["src/api/user.ts", "src/ui/user.ts", "src/auth.go", "src/b.rs", "tests/a.rs", "Cargo.lock", "README.md"]
        );
    }
}

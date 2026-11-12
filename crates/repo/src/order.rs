//! Review order: sources first, each followed by its tests; then tests with
//! no matching source; then generated, lock and vendored files.

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

fn common_dirs(a: &str, b: &str) -> usize {
    split(a).0.iter().zip(split(b).0.iter()).take_while(|(x, y)| x == y).count()
}

/// Returns indices into `paths` in review order, with each file's kind.
/// Returns `(index, kind, paired)`; `paired` marks a test placed right after
/// the source it covers.
pub fn smart_order(paths: &[&str]) -> Vec<(usize, FileKind, bool)> {
    let kinds: Vec<FileKind> = paths.iter().map(|p| classify(p)).collect();
    let mut sources: Vec<usize> = (0..paths.len()).filter(|&i| kinds[i] == FileKind::Source).collect();
    let mut tests: Vec<usize> = (0..paths.len()).filter(|&i| kinds[i] == FileKind::Test).collect();
    let mut generated: Vec<usize> = (0..paths.len()).filter(|&i| kinds[i] == FileKind::Generated).collect();
    for v in [&mut sources, &mut tests, &mut generated] {
        v.sort_by_key(|&i| paths[i]);
    }

    // Attach each test to the source with the same stem sharing the most directories.
    let mut attached: Vec<Vec<usize>> = vec![Vec::new(); paths.len()];
    let mut orphans = Vec::new();
    for &t in &tests {
        let name = split(paths[t]).1;
        let want = test_stem(name).unwrap_or_else(|| stem(name));
        let best = sources
            .iter()
            .filter(|&&s| stem(split(paths[s]).1) == want)
            .max_by_key(|&&s| (common_dirs(paths[s], paths[t]), std::cmp::Reverse(s)));
        match best {
            Some(&s) => attached[s].push(t),
            None => orphans.push(t),
        }
    }

    let mut out = Vec::with_capacity(paths.len());
    for s in sources {
        out.push((s, FileKind::Source, false));
        out.extend(attached[s].iter().map(|&t| (t, FileKind::Test, true)));
    }
    out.extend(orphans.into_iter().map(|t| (t, FileKind::Test, false)));
    out.extend(generated.into_iter().map(|g| (g, FileKind::Generated, false)));
    out
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
    fn tests_follow_their_source() {
        let paths = [
            "Cargo.lock",
            "src/api/user.ts",
            "src/api/user.test.ts",
            "src/ui/user.ts",
            "src/ui/__tests__/user.test.tsx",
            "src/auth.go",
            "src/auth_test.go",
            "tests/test_billing.py",
            "README.md",
            "src/billing.py",
            "tests/integration.rs",
        ];
        let order: Vec<&str> = smart_order(&paths).into_iter().map(|(i, _, _)| paths[i]).collect();
        assert_eq!(
            order,
            [
                "README.md",
                "src/api/user.ts",
                "src/api/user.test.ts",
                "src/auth.go",
                "src/auth_test.go",
                "src/billing.py",
                "tests/test_billing.py",
                "src/ui/user.ts",
                "src/ui/__tests__/user.test.tsx",
                "tests/integration.rs",
                "Cargo.lock",
            ]
        );
    }
}

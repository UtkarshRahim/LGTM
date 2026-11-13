use std::fmt::Write;

pub struct ChangedFile {
    pub path: String,
    pub status: &'static str,
    /// Inclusive line ranges in the new file that the diff shows.
    pub ranges: Vec<(u32, u32)>,
}

pub struct PrContext<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub head_ref: &'a str,
    pub base_ref: &'a str,
    pub head_sha: &'a str,
    pub merge_base: &'a str,
    /// Relative to the checkout.
    pub diff_path: &'a str,
    pub files: &'a [ChangedFile],
}

const MAX_FILES: usize = 400;
const MAX_DESCRIPTION: usize = 4000;

pub fn prompt(skill_name: &str, skill_body: &str, pr: &PrContext<'_>) -> String {
    let mut p = String::new();
    let _ = writeln!(
        p,
        "You are running the review skill \"{skill_name}\" on a pull request. This is a read-only review: \
         don't edit files and don't run commands that change anything.\n"
    );
    let _ = writeln!(p, "<skill>\n{skill_body}\n</skill>\n");
    let _ = writeln!(p, "## Pull request\n\nTitle: {}\nBranch: {} → {}", pr.title, pr.head_ref, pr.base_ref);
    let description = pr.description.trim();
    if !description.is_empty() {
        let cut: String = description.chars().take(MAX_DESCRIPTION).collect();
        let more = if cut.len() < description.len() { "\n[…]" } else { "" };
        let _ = writeln!(p, "\nDescription:\n{cut}{more}");
    }
    let _ = writeln!(
        p,
        "\nThe working directory is a checkout of the PR head ({}). The full diff against the merge base ({}) is in `{}`.\n\
         Changed files, with the line ranges of the new file that the diff shows:\n",
        pr.head_sha, pr.merge_base, pr.diff_path
    );
    for f in pr.files.iter().take(MAX_FILES) {
        let ranges: Vec<String> =
            f.ranges.iter().map(|&(a, b)| if a == b { a.to_string() } else { format!("{a}-{b}") }).collect();
        let ranges = if ranges.is_empty() { "(no lines)".to_owned() } else { ranges.join(", ") };
        let _ = writeln!(p, "- {} ({}): {ranges}", f.path, f.status);
    }
    if pr.files.len() > MAX_FILES {
        let _ = writeln!(p, "- …and {} more; see the diff file", pr.files.len() - MAX_FILES);
    }
    p.push_str(CONTRACT);
    p
}

const CONTRACT: &str = r#"
## Output

Write the review the skill asks for, in full. Keep every section it names, including ones that are not line comments: the approach, test gaps, risks, and the rest. Do not shorten the review down to code changes.

After that writeup, with nothing after it, add the line comments as this JSON object in a ```json block. "line" and "end_line" are line numbers in the new file, inside the ranges above. If something outside them matters, attach it to the nearest changed line. Leave out praise and anything you aren't confident about.

{
  "findings": [
    {
      "path": "relative/path/to/file",
      "line": 12,
      "end_line": 14,
      "severity": "blocker | warning | nit",
      "title": "One-line summary",
      "body": "Why it matters and what to do, in Markdown",
      "suggestion": "Optional replacement code for lines line..end_line"
    }
  ]
}

Use "blocker" for bugs, security problems and data loss; "warning" for things that should change before merging; "nit" for style and small improvements. Omit "end_line" and "suggestion" when they don't apply. If there is no line comment, use {"findings": []}. The writeup above is still kept when the list is empty.
"#;

/// Sent in the same session when the reply couldn't be read.
pub fn retry_prompt(error: &str) -> String {
    format!(
        "Your reply couldn't be read as findings ({error}). Write the full review again, then the JSON object \
         in a ```json block, and nothing after it."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_files_and_ranges() {
        let files = [
            ChangedFile { path: "src/a.rs".into(), status: "modified", ranges: vec![(3, 9), (20, 20)] },
            ChangedFile { path: "old.rs".into(), status: "deleted", ranges: vec![] },
        ];
        let p = prompt("review", "Look for bugs.", &PrContext {
            title: "Fix",
            description: "",
            head_ref: "fix",
            base_ref: "main",
            head_sha: "abc",
            merge_base: "def",
            diff_path: ".prism/pr.diff",
            files: &files,
        });
        assert!(p.contains("<skill>\nLook for bugs.\n</skill>"));
        assert!(p.contains("- src/a.rs (modified): 3-9, 20\n- old.rs (deleted): (no lines)"));
        assert!(!p.contains("Description:"));
        assert!(p.contains("\"findings\""));
    }
}

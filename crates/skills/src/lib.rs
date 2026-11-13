//! Review skills: discovering `SKILL.md` files, turning one into a prompt for
//! a pull request, and reading the findings the agent sends back.

mod findings;
mod prompt;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use findings::{Finding, Severity, parse_findings, parse_reply};
pub use prompt::{ChangedFile, PrContext, prompt, retry_prompt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// Checked into the repository under review.
    Repo,
    Personal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct Skill {
    /// Stable across launches: `repo:<dir>` or `personal:<dir>`.
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    /// Folder holding `SKILL.md`.
    pub dir: String,
}

/// Where repositories keep skills, relative to their root.
pub const REPO_SKILL_DIRS: [&str; 3] = [".claude/skills", ".cursor/skills", ".agents/skills"];

/// Default personal skill folders, relative to the home directory.
pub const PERSONAL_SKILL_DIRS: [&str; 3] = [".claude/skills", ".cursor/skills", ".agents/skills"];

/// Skills from the repository (if any) and personal folders. A repo skill
/// hides a personal one with the same name; within a source, the first
/// folder listed wins.
pub fn discover(repo_root: Option<&Path>, personal_dirs: &[PathBuf]) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();
    let mut add = |dir: &Path, source: SkillSource| {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut found: Vec<Skill> = entries
            .flatten()
            .filter_map(|e| read_skill(&e.path(), source))
            .filter(|s| !out.iter().any(|o| o.name == s.name))
            .collect();
        found.sort_by_key(|s| s.name.to_lowercase());
        out.extend(found);
    };
    if let Some(root) = repo_root {
        for d in REPO_SKILL_DIRS {
            add(&root.join(d), SkillSource::Repo);
        }
    }
    for d in personal_dirs {
        add(d, SkillSource::Personal);
    }
    out
}

fn read_skill(dir: &Path, source: SkillSource) -> Option<Skill> {
    let text = std::fs::read_to_string(dir.join("SKILL.md")).ok()?;
    let folder = dir.file_name()?.to_string_lossy().into_owned();
    let (meta, _) = split_frontmatter(&text);
    let field = |k: &str| meta.iter().find(|(key, _)| *key == k).map(|(_, v)| v.clone());
    let prefix = match source {
        SkillSource::Repo => "repo",
        SkillSource::Personal => "personal",
    };
    Some(Skill {
        id: format!("{prefix}:{folder}"),
        name: field("name").unwrap_or_else(|| folder.clone()),
        description: field("description").unwrap_or_default(),
        source,
        dir: dir.to_string_lossy().into_owned(),
    })
}

/// The instructions in `SKILL.md`, without the frontmatter.
pub fn load_body(dir: &Path) -> std::io::Result<String> {
    let text = std::fs::read_to_string(dir.join("SKILL.md"))?;
    Ok(split_frontmatter(&text).1.trim().to_owned())
}

/// Top-level `key: value` pairs of a `---` block, and the rest of the text.
fn split_frontmatter(text: &str) -> (Vec<(&str, String)>, &str) {
    let Some(rest) = text.strip_prefix("---").and_then(|r| r.strip_prefix('\n').or_else(|| r.strip_prefix("\r\n"))) else {
        return (Vec::new(), text);
    };
    let Some(end) = rest.find("\n---") else { return (Vec::new(), text) };
    let block = &rest[..end];
    let body = rest[end + 4..].trim_start_matches(['-']).trim_start_matches(['\r', '\n']);
    let meta = block
        .lines()
        .filter(|l| !l.starts_with([' ', '\t']))
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim(), unquote(v.trim())))
        .collect();
    (meta, body)
}

fn unquote(v: &str) -> String {
    let v = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')).or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\''))).unwrap_or(v);
    v.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, folder: &str, text: &str) {
        std::fs::create_dir_all(dir.join(folder)).unwrap();
        std::fs::write(dir.join(folder).join("SKILL.md"), text).unwrap();
    }

    #[test]
    fn discovers_and_shadows() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let home = tmp.path().join("home");
        write(&repo.join(".claude/skills"), "review", "---\nname: review\ndescription: \"Repo review\"\n---\nCheck it.\n");
        write(&home, "review", "---\nname: review\ndescription: mine\n---\nx");
        write(&home, "Security", "---\nname: security\ndescription: Find holes\nlicense: MIT\n---\n\nLook hard.");
        write(&home, "bare", "No frontmatter here.");
        std::fs::create_dir_all(home.join("empty")).unwrap();

        let skills = discover(Some(&repo), std::slice::from_ref(&home));
        let names: Vec<_> = skills.iter().map(|s| (s.id.as_str(), s.name.as_str(), s.source)).collect();
        assert_eq!(
            names,
            [
                ("repo:review", "review", SkillSource::Repo),
                ("personal:bare", "bare", SkillSource::Personal),
                ("personal:Security", "security", SkillSource::Personal),
            ]
        );
        assert_eq!(skills[0].description, "Repo review");
        assert_eq!(load_body(Path::new(&skills[2].dir)).unwrap(), "Look hard.");
        assert_eq!(load_body(Path::new(&skills[1].dir)).unwrap(), "No frontmatter here.");
    }
}

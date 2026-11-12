use serde::Serialize;

/// How to start an ACP agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct AgentSpec {
    pub id: String,
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
}

struct Entry {
    id: &'static str,
    name: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    /// The CLI that must be installed (and signed in) for the agent to work.
    requires: &'static str,
}

const CATALOG: [Entry; 4] = [
    Entry { id: "cursor", name: "Cursor", program: "cursor-agent", args: &["acp"], requires: "cursor-agent" },
    Entry {
        id: "claude",
        name: "Claude Code",
        program: "npx",
        args: &["-y", "@zed-industries/claude-code-acp"],
        requires: "claude",
    },
    Entry { id: "gemini", name: "Gemini CLI", program: "gemini", args: &["--experimental-acp"], requires: "gemini" },
    Entry { id: "codex", name: "Codex", program: "npx", args: &["-y", "@zed-industries/codex-acp"], requires: "codex" },
];

/// Known agents, each with whether its CLI is on the login-shell `PATH`.
pub fn catalog() -> Vec<(AgentSpec, bool)> {
    let path = prism_env::var("PATH").unwrap_or_default();
    CATALOG
        .iter()
        .map(|e| {
            let spec = AgentSpec {
                id: e.id.into(),
                name: e.name.into(),
                program: e.program.into(),
                args: e.args.iter().map(|a| (*a).into()).collect(),
            };
            (spec, on_path(e.requires, &path))
        })
        .collect()
}

fn on_path(program: &str, path: &str) -> bool {
    std::env::split_paths(path).any(|dir| {
        let p = dir.join(program);
        p.metadata().is_ok_and(|m| m.is_file() && std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o111 != 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_programs_on_path() {
        assert!(on_path("sh", "/usr/bin:/bin"));
        assert!(!on_path("definitely-not-a-program", "/usr/bin:/bin"));
        assert_eq!(catalog().len(), 4);
    }
}

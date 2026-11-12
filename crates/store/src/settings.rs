use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("reading settings: {0}")]
    Io(#[from] std::io::Error),
    #[error("parsing settings: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("writing settings: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(default)]
pub struct Settings {
    pub ui: UiSettings,
    pub github: GithubSettings,
    pub skills: SkillSettings,
    pub checkout: CheckoutSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(default)]
pub struct UiSettings {
    pub ui_font: String,
    pub code_font: String,
    pub diff_layout: DiffLayout,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DiffLayout {
    #[default]
    Unified,
    Split,
}


#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, specta::Type)]
#[serde(default)]
pub struct GithubSettings {
    /// `host/login` of the account to prefer when several are signed in.
    pub default_account: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(default)]
pub struct SkillSettings {
    /// Personal skill folders; `~` is the home directory.
    pub dirs: Vec<String>,
    /// Skill id preselected in the palette.
    pub pinned: Option<String>,
    /// Agent id skills run with.
    pub agent: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DirtyCheckout {
    #[default]
    Ask,
    Worktree,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, specta::Type)]
#[serde(default)]
pub struct CheckoutSettings {
    pub when_dirty: DirtyCheckout,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ui: UiSettings::default(),
            github: GithubSettings::default(),
            skills: SkillSettings::default(),
            checkout: CheckoutSettings::default(),
        }
    }
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            ui_font: "Inter".into(),
            code_font: "JetBrains Mono".into(),
            diff_layout: DiffLayout::default(),
        }
    }
}

impl Default for SkillSettings {
    fn default() -> Self {
        Self {
            dirs: vec!["~/.claude/skills".into(), "~/.cursor/skills".into(), "~/.agents/skills".into()],
            pinned: None,
            agent: "cursor".into(),
        }
    }
}

impl Settings {
    /// Loads settings, writing defaults if the file does not exist yet.
    pub fn load_or_init(path: &Path) -> Result<Self, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let settings = Self::default();
                settings.save(path)?;
                Ok(settings)
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_file_fills_defaults() {
        let s: Settings = toml::from_str("[ui]\ncode_font = \"Fira Code\"\n").unwrap();
        assert_eq!(s.ui.code_font, "Fira Code");
        assert_eq!(s.ui.ui_font, "Inter");
        assert_eq!(s.skills.dirs[0], "~/.claude/skills");
        assert_eq!(s.skills.agent, "cursor");
        let s: Settings = toml::from_str("[skills]\ndirs = [\"~/x\"]\n").unwrap();
        assert_eq!((s.skills.dirs.len(), s.skills.agent.as_str()), (1, "cursor"));
    }

    #[test]
    fn init_then_roundtrip() {
        let dir = std::env::temp_dir().join(format!("prism-settings-{}", std::process::id()));
        let path = dir.join("settings.toml");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Settings::load_or_init(&path).unwrap();
        assert_eq!(s, Settings::default());
        s.checkout.when_dirty = DirtyCheckout::Worktree;
        s.save(&path).unwrap();
        assert_eq!(Settings::load_or_init(&path).unwrap(), s);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

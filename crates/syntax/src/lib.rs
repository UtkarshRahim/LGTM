//! Tree-sitter highlighting for diff lines.
//!
//! Diffs only contain hunks, so each side (old: context + deletions, new:
//! context + additions) is reassembled into one text and highlighted as a
//! whole. Constructs that start outside a hunk (e.g. a block comment opened
//! above it) can be misread; that is the price of not fetching whole files.

use std::collections::HashMap;
use std::sync::OnceLock;

use tree_sitter::Language;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Capture names we color. Index into this list is the class sent to the UI.
pub const CLASSES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
    "function",
    "function.builtin",
    "keyword",
    "label",
    "number",
    "operator",
    "property",
    "punctuation",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable.builtin",
    "variable.parameter",
];

const MAX_LINE: usize = 2_000;
const MAX_LINES: usize = 30_000;

/// Spans for one line: flat `[start, end, class, start, end, class, ...]`,
/// offsets in UTF-16 code units so the UI can slice JS strings directly.
pub type LineSpans = Vec<u32>;

struct Lang {
    config: HighlightConfiguration,
}

fn config(name: &'static str, lang: Language, highlights: &[&str], injections: &str, locals: &str) -> Lang {
    let mut config = HighlightConfiguration::new(lang, name, &highlights.join("\n"), injections, locals)
        .unwrap_or_else(|e| panic!("bad {name} highlight query: {e}"));
    config.configure(CLASSES);
    Lang { config }
}

fn build(name: &str) -> Option<Lang> {
    let js = tree_sitter_javascript::HIGHLIGHT_QUERY;
    let jsx = tree_sitter_javascript::JSX_HIGHLIGHT_QUERY;
    let ts = tree_sitter_typescript::HIGHLIGHTS_QUERY;
    let c = tree_sitter_c::HIGHLIGHT_QUERY;
    Some(match name {
        "rust" => config("rust", tree_sitter_rust::LANGUAGE.into(), &[tree_sitter_rust::HIGHLIGHTS_QUERY], tree_sitter_rust::INJECTIONS_QUERY, ""),
        "javascript" => config("javascript", tree_sitter_javascript::LANGUAGE.into(), &[js, jsx], tree_sitter_javascript::INJECTIONS_QUERY, tree_sitter_javascript::LOCALS_QUERY),
        "typescript" => config("typescript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), &[ts, js], "", tree_sitter_typescript::LOCALS_QUERY),
        "tsx" => config("tsx", tree_sitter_typescript::LANGUAGE_TSX.into(), &[ts, js, jsx], "", tree_sitter_typescript::LOCALS_QUERY),
        "python" => config("python", tree_sitter_python::LANGUAGE.into(), &[tree_sitter_python::HIGHLIGHTS_QUERY], "", ""),
        "go" => config("go", tree_sitter_go::LANGUAGE.into(), &[tree_sitter_go::HIGHLIGHTS_QUERY], "", ""),
        "json" => config("json", tree_sitter_json::LANGUAGE.into(), &[tree_sitter_json::HIGHLIGHTS_QUERY], "", ""),
        "css" => config("css", tree_sitter_css::LANGUAGE.into(), &[tree_sitter_css::HIGHLIGHTS_QUERY], "", ""),
        "html" => config("html", tree_sitter_html::LANGUAGE.into(), &[tree_sitter_html::HIGHLIGHTS_QUERY], tree_sitter_html::INJECTIONS_QUERY, ""),
        "bash" => config("bash", tree_sitter_bash::LANGUAGE.into(), &[tree_sitter_bash::HIGHLIGHT_QUERY], "", ""),
        "toml" => config("toml", tree_sitter_toml_ng::LANGUAGE.into(), &[tree_sitter_toml_ng::HIGHLIGHTS_QUERY], "", ""),
        "yaml" => config("yaml", tree_sitter_yaml::LANGUAGE.into(), &[tree_sitter_yaml::HIGHLIGHTS_QUERY], "", ""),
        "c" => config("c", tree_sitter_c::LANGUAGE.into(), &[c], "", ""),
        "cpp" => config("cpp", tree_sitter_cpp::LANGUAGE.into(), &[tree_sitter_cpp::HIGHLIGHT_QUERY, c], "", ""),
        "java" => config("java", tree_sitter_java::LANGUAGE.into(), &[tree_sitter_java::HIGHLIGHTS_QUERY], "", ""),
        "ruby" => config("ruby", tree_sitter_ruby::LANGUAGE.into(), &[tree_sitter_ruby::HIGHLIGHTS_QUERY], "", tree_sitter_ruby::LOCALS_QUERY),
        _ => return None,
    })
}

const NAMES: &[&str] = &[
    "rust", "javascript", "typescript", "tsx", "python", "go", "json", "css", "html", "bash", "toml", "yaml", "c", "cpp", "java", "ruby",
];

/// Compiling a grammar's queries is slow (tens of ms), so each is built on first use.
fn get(name: &str) -> Option<&'static Lang> {
    static LANGS: OnceLock<HashMap<&'static str, OnceLock<Option<Lang>>>> = OnceLock::new();
    let langs = LANGS.get_or_init(|| NAMES.iter().map(|n| (*n, OnceLock::new())).collect());
    langs.get(name)?.get_or_init(|| build(name)).as_ref()
}

/// Picks a grammar from the file name.
pub fn language_for(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    Some(match (name, ext.as_deref()) {
        ("Cargo.lock", _) | (_, Some("toml")) => "toml",
        (_, Some("rs")) => "rust",
        (_, Some("js" | "mjs" | "cjs" | "jsx")) => "javascript",
        (_, Some("ts" | "mts" | "cts")) => "typescript",
        (_, Some("tsx")) => "tsx",
        (_, Some("py" | "pyi")) => "python",
        (_, Some("go")) => "go",
        (_, Some("json" | "jsonc")) => "json",
        (_, Some("css" | "scss")) => "css",
        (_, Some("html" | "htm")) => "html",
        ("Dockerfile" | "Makefile", _) => return None,
        (_, Some("sh" | "bash" | "zsh")) | (".bashrc" | ".zshrc", _) => "bash",
        (_, Some("yml" | "yaml")) => "yaml",
        (_, Some("c" | "h")) => "c",
        (_, Some("cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx")) => "cpp",
        (_, Some("java")) => "java",
        (_, Some("rb")) | ("Gemfile" | "Rakefile", _) => "ruby",
        _ => return None,
    })
}

/// Highlights `lines` as one document in `lang`. Returns one entry per input
/// line (empty when nothing to color), or `None` if the input is too large.
pub fn highlight_lines(lang: &str, lines: &[&str]) -> Option<Vec<LineSpans>> {
    if lines.len() > MAX_LINES || lines.iter().any(|l| l.len() > MAX_LINE) {
        return None;
    }
    let lang = get(lang)?;
    let source = lines.join("\n");

    let mut line_starts = Vec::with_capacity(lines.len());
    let mut pos = 0;
    for l in lines {
        line_starts.push(pos);
        pos += l.len() + 1;
    }

    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(&lang.config, source.as_bytes(), None, None, |name| get(name).map(|l| &l.config))
        .ok()?;

    let mut out: Vec<LineSpans> = vec![Vec::new(); lines.len()];
    let mut stack: Vec<usize> = Vec::new();
    for event in events {
        match event.ok()? {
            HighlightEvent::HighlightStart(h) => stack.push(h.0),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let Some(&class) = stack.last() else { continue };
                push_span(&mut out, lines, &line_starts, start, end, class as u32);
            }
        }
    }
    Some(out)
}

/// Splits a byte range of the joined source into per-line UTF-16 spans.
fn push_span(out: &mut [LineSpans], lines: &[&str], starts: &[usize], start: usize, end: usize, class: u32) {
    let first = starts.partition_point(|&s| s <= start).saturating_sub(1);
    for i in first..lines.len() {
        let ls = starts[i];
        if ls >= end {
            break;
        }
        let line = lines[i];
        let a = start.saturating_sub(ls).min(line.len());
        let b = (end - ls).min(line.len());
        if a < b && line.is_char_boundary(a) && line.is_char_boundary(b) {
            let a16 = line[..a].encode_utf16().count() as u32;
            let b16 = a16 + line[a..b].encode_utf16().count() as u32;
            out[i].extend([a16, b16, class]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(name: &str) -> u32 {
        CLASSES.iter().position(|c| *c == name).unwrap() as u32
    }

    fn spans_named(spans: &LineSpans, line: &str) -> Vec<(String, &'static str)> {
        spans
            .chunks(3)
            .map(|s| {
                let text: String = line.encode_utf16().collect::<Vec<_>>()[s[0] as usize..s[1] as usize]
                    .iter()
                    .map(|&u| char::from_u32(u as u32).unwrap_or('?'))
                    .collect();
                (text, CLASSES[s[2] as usize])
            })
            .collect()
    }

    #[test]
    fn every_grammar_loads() {
        for name in NAMES {
            assert!(get(name).is_some(), "{name}");
        }
    }

    #[test]
    fn rust_keywords_strings_comments() {
        let lines = ["fn main() {", "    let s = \"hi\"; // greet", "}"];
        let out = highlight_lines("rust", &lines).unwrap();
        let l1 = spans_named(&out[1], lines[1]);
        assert!(l1.contains(&("let".into(), "keyword")), "{l1:?}");
        assert!(l1.contains(&("\"hi\"".into(), "string")), "{l1:?}");
        assert!(l1.contains(&("// greet".into(), "comment")), "{l1:?}");
        assert!(out[0].chunks(3).any(|s| s[2] == class("keyword")));
    }

    #[test]
    fn multiline_comment_spans_lines() {
        let lines = ["/* a", "b */ const x = 1;"];
        let out = highlight_lines("typescript", &lines).unwrap();
        assert_eq!(spans_named(&out[0], lines[0])[0], ("/* a".into(), "comment"));
        assert_eq!(spans_named(&out[1], lines[1])[0], ("b */".into(), "comment"));
    }

    #[test]
    fn utf16_offsets() {
        let lines = ["let é = \"ü\";"];
        let out = highlight_lines("javascript", &lines).unwrap();
        assert!(spans_named(&out[0], lines[0]).contains(&("\"ü\"".into(), "string")));
    }

    #[test]
    fn detects_languages() {
        assert_eq!(language_for("src/App.tsx"), Some("tsx"));
        assert_eq!(language_for("Cargo.lock"), Some("toml"));
        assert_eq!(language_for(".github/workflows/ci.yml"), Some("yaml"));
        assert_eq!(language_for("README.md"), None);
    }
}

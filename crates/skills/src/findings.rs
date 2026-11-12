use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Blocker,
    Warning,
    Nit,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blocker => "blocker",
            Self::Warning => "warning",
            Self::Nit => "nit",
        }
    }

    /// Accepts the contract's words and the usual synonyms agents reach for.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "blocker" | "blocking" | "critical" | "error" | "high" | "major" | "bug" => Some(Self::Blocker),
            "warning" | "warn" | "medium" | "moderate" | "issue" => Some(Self::Warning),
            "nit" | "nitpick" | "minor" | "low" | "info" | "style" | "suggestion" => Some(Self::Nit),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct Finding {
    pub path: String,
    /// Line in the new version of the file; `None` for file-level findings.
    pub line: Option<u32>,
    pub end_line: Option<u32>,
    pub severity: Severity,
    pub title: String,
    pub body: String,
    /// Replacement for lines `line..=end_line`.
    pub suggestion: Option<String>,
}

/// Reads the findings JSON out of an agent's final message. `root` is the
/// checkout the agent ran in, stripped from absolute paths.
pub fn parse_findings(text: &str, root: &str) -> Result<Vec<Finding>, String> {
    let mut last_err = "no JSON object with a \"findings\" list found".to_owned();
    for candidate in candidates(text) {
        match serde_json::from_str::<Value>(candidate) {
            Ok(v) => match findings_from(&v, root) {
                Ok(f) => return Ok(f),
                Err(e) => last_err = e,
            },
            Err(e) => last_err = format!("invalid JSON: {e}"),
        }
    }
    Err(last_err)
}

/// Fenced blocks (last first), then the whole text, then the outermost braces.
fn candidates(text: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let Some(nl) = after.find('\n') else { break };
        let lang = after[..nl].trim();
        let body = &after[nl + 1..];
        let Some(end) = body.find("```") else { break };
        if lang.is_empty() || lang.eq_ignore_ascii_case("json") {
            blocks.push(body[..end].trim());
        }
        rest = &body[end + 3..];
    }
    blocks.reverse();
    blocks.push(text.trim());
    if let (Some(a), Some(b)) = (text.find('{'), text.rfind('}'))
        && a < b
    {
        blocks.push(&text[a..=b]);
    }
    blocks
}

fn findings_from(v: &Value, root: &str) -> Result<Vec<Finding>, String> {
    let list = match v {
        Value::Array(a) => a,
        Value::Object(o) => o.get("findings").and_then(Value::as_array).ok_or("missing \"findings\" list")?,
        _ => return Err("expected an object with a \"findings\" list".into()),
    };
    list.iter().enumerate().map(|(i, f)| finding(f, root).map_err(|e| format!("findings[{i}]: {e}"))).collect()
}

fn finding(v: &Value, root: &str) -> Result<Finding, String> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let n = |k: &str| v.get(k).and_then(|x| x.as_u64().or_else(|| x.as_str()?.trim().parse().ok())).map(|x| x as u32).filter(|&x| x > 0);
    let path = s("path").or_else(|| s("file")).ok_or("missing \"path\"")?;
    let path = path.strip_prefix(root).unwrap_or(path).trim_start_matches('/').trim_start_matches("./");
    let severity = s("severity").ok_or("missing \"severity\"")?;
    let severity = Severity::parse(severity).ok_or_else(|| format!("unknown severity {severity:?}; use blocker, warning or nit"))?;
    let title = s("title").ok_or("missing \"title\"")?;
    let line = n("line").or_else(|| n("start_line"));
    let end_line = n("end_line").filter(|&e| line.is_some_and(|l| e > l));
    Ok(Finding {
        path: path.to_owned(),
        line,
        end_line,
        severity,
        title: title.to_owned(),
        body: s("body").or_else(|| s("description")).unwrap_or("").to_owned(),
        suggestion: v.get("suggestion").and_then(Value::as_str).filter(|x| !x.trim().is_empty()).map(|x| x.trim_end_matches('\n').to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_last_json_block() {
        let text = "Looked around.\n```json\n{\"findings\": []}\n```\nFinal:\n```json\n{\"findings\": [\
            {\"path\": \"/tmp/wt/src/a.rs\", \"line\": 4, \"end_line\": 6, \"severity\": \"Critical\", \"title\": \"Off by one\", \
             \"body\": \"`i <= n`\", \"suggestion\": \"for i in 0..n {\\n\"}]}\n```";
        let f = parse_findings(text, "/tmp/wt").unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].path.as_str(), f[0].line, f[0].end_line, f[0].severity), ("src/a.rs", Some(4), Some(6), Severity::Blocker));
        assert_eq!(f[0].suggestion.as_deref(), Some("for i in 0..n {"));
    }

    #[test]
    fn bare_json_and_lenient_fields() {
        let f = parse_findings(r#"[{"file": "./x.go", "line": "9", "end_line": 9, "severity": "minor", "title": "Name"}]"#, "/r").unwrap();
        assert_eq!((f[0].path.as_str(), f[0].line, f[0].end_line, f[0].severity), ("x.go", Some(9), None, Severity::Nit));
        let f = parse_findings("Here you go: {\"findings\": []} hope it helps", "/r").unwrap();
        assert!(f.is_empty());
    }

    #[test]
    fn explains_what_is_wrong() {
        assert!(parse_findings("I found nothing.", "/r").is_err());
        let e = parse_findings(r#"{"findings": [{"path": "a", "severity": "meh", "title": "t"}]}"#, "/r").unwrap_err();
        assert!(e.contains("findings[0]") && e.contains("meh"), "{e}");
    }
}

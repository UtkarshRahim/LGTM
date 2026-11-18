//! File text from the GitHub contents API, sliced to a line window.
//! The contents API returns the whole blob, base64-encoded, up to 1MB.

use base64::Engine;
use serde::Deserialize;
use serde::Serialize;

use crate::client::GitHub;
use crate::error::{GhError, GhResult};
use crate::query::PrRef;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct FileSlice {
    pub lines: Vec<String>,
    /// True when `end` reached the last line of the file, or past it.
    pub eof: bool,
}

/// 1-based inclusive. A start past the end yields no lines and `eof`.
pub fn slice_lines(text: &str, start: u32, end: u32) -> FileSlice {
    if start == 0 || end < start {
        return FileSlice {
            lines: Vec::new(),
            eof: true,
        };
    }
    let mut lines = Vec::new();
    let mut n = 0u32;
    for raw in text.split_inclusive('\n') {
        n += 1;
        if n < start {
            continue;
        }
        if n > end {
            break;
        }
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        lines.push(line.strip_suffix('\r').unwrap_or(line).to_owned());
    }
    FileSlice {
        lines,
        eof: n <= end,
    }
}

pub fn decode_content(encoding: Option<&str>, content: &str) -> Result<String, String> {
    if encoding.is_some_and(|e| e != "base64") {
        return Err(format!("unsupported encoding {encoding:?}"));
    }
    let cleaned: String = content.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cleaned)
        .map_err(|e| e.to_string())?;
    if bytes.contains(&0) {
        return Err("binary file".into());
    }
    String::from_utf8(bytes).map_err(|_| "file is not UTF-8".into())
}

/// The file at `sha` as UTF-8 text. Fails for binaries, directories, and blobs
/// GitHub won't return inline.
pub async fn file_text(gh: &GitHub, pr: &PrRef, path: &str, sha: &str) -> GhResult<String> {
    let path_enc = encode_path(path);
    let url = format!(
        "/repos/{}/{}/contents/{path_enc}?ref={sha}",
        pr.owner, pr.repo
    );
    let file: Contents = gh.rest_get(&url).await?;
    if file.kind.as_deref().is_some_and(|k| k != "file") {
        return Err(GhError::api("Not a file"));
    }
    let content = file
        .content
        .ok_or_else(|| GhError::api("GitHub didn't return the file contents"))?;
    decode_content(file.encoding.as_deref(), &content).map_err(GhError::api)
}

fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|seg| {
            let mut out = String::new();
            for b in seg.bytes() {
                match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                        out.push(b as char)
                    }
                    _ => out.push_str(&format!("%{b:02X}")),
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[derive(Deserialize)]
struct Contents {
    #[serde(rename = "type")]
    kind: Option<String>,
    encoding: Option<String>,
    content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_a_window_and_reports_eof() {
        let mid = slice_lines("a\nb\nc\n", 2, 3);
        assert_eq!(mid.lines, vec!["b".to_string(), "c".to_string()]);
        assert!(mid.eof);
        let short = slice_lines("a\nb\nc\n", 2, 5);
        assert_eq!(short.lines, vec!["b".to_string(), "c".to_string()]);
        assert!(short.eof);
        let more = slice_lines("a\nb\nc\nd", 1, 2);
        assert_eq!(more.lines, vec!["a".to_string(), "b".to_string()]);
        assert!(!more.eof);
        let past = slice_lines("a\n", 4, 6);
        assert!(past.lines.is_empty());
        assert!(past.eof);
    }

    #[test]
    fn decodes_github_content() {
        assert_eq!(decode_content(Some("base64"), "aGkK").unwrap(), "hi\n");
        assert_eq!(decode_content(Some("base64"), "aGkK\n").unwrap(), "hi\n");
        assert!(decode_content(Some("base64"), "AAE=").is_err());
    }
}

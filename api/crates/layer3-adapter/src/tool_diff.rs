//! Normalize reported file diffs into the existing bounded textual output surface.
use crate::ToolCallContent;
use similar::{ChangeTag, TextDiff};
use std::fmt::Write as _;

const LIMIT: usize = 16 * 1024;
struct BoundedText {
    text: String,
    truncated: bool,
}
impl std::fmt::Write for BoundedText {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        let mut end = value.len().min(LIMIT.saturating_sub(self.text.len()));
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&value[..end]);
        if end < value.len() {
            self.truncated = true;
            Err(std::fmt::Error)
        } else {
            Ok(())
        }
    }
}

pub(crate) fn reported_diffs(content: &[ToolCallContent]) -> Option<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(50);
    let diffs: Vec<_> = content
        .iter()
        .filter_map(|value| match value {
            ToolCallContent::Diff(diff) => Some(diff),
            _ => None,
        })
        .map(|value| {
            let diff = TextDiff::configure()
                .deadline(deadline)
                .diff_lines(value.old_text.as_deref().unwrap_or(""), &value.new_text);
            let mut added = 0_u64;
            let mut removed = 0_u64;
            for change in diff.iter_all_changes() {
                match change.tag() {
                    ChangeTag::Insert => added += 1,
                    ChangeTag::Delete => removed += 1,
                    ChangeTag::Equal => (),
                }
            }
            (
                serde_json::to_string(&value.path.to_string_lossy())
                    .expect("file path is serializable"),
                diff,
                added,
                removed,
            )
        })
        .collect();
    if diffs.is_empty() {
        return None;
    }
    let mut output = BoundedText {
        text: String::new(),
        truncated: false,
    };
    let added: u64 = diffs.iter().map(|(_, _, added, _)| added).sum();
    let removed: u64 = diffs.iter().map(|(_, _, _, removed)| removed).sum();
    let _ = writeln!(output, "File changes: {} +{added} −{removed}", diffs.len());
    for (path, _, added, removed) in &diffs {
        let _ = writeln!(output, "File: {path} +{added} −{removed}");
    }
    for (path, diff, _, _) in &diffs {
        let _ = write!(output, "{}", diff.unified_diff().header(path, path));
    }
    // The core's unchanged 16 KiB budget remains authoritative, including its
    // existing truncation flag. Four-byte UTF-8 boundaries cannot hide overflow.
    if output.truncated {
        output.text.push_str("\n[truncated]");
    }
    Some(output.text)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fields(values: serde_json::Value) -> crate::ToolCallUpdateFields {
        serde_json::from_value(serde_json::json!({"content":values})).unwrap()
    }
    #[test]
    fn projects_added_updated_and_deleted_files_with_exact_line_counts() {
        let fields = fields(serde_json::json!([
            {"type":"diff","path":"/home/cma/app.js","oldText":"same\nold\n","newText":"same\nnew\nextra\n"},
            {"type":"diff","path":"/home/cma/new.txt","newText":"created\n"},
            {"type":"diff","path":"/home/cma/old.txt","oldText":"removed\n","newText":""}
        ]));
        let values = crate::reported_tool_text(&fields);
        let text = values[0].as_ref();
        assert!(text.starts_with("File changes: 3 +3 −2\n"));
        assert!(text.contains("File: \"/home/cma/app.js\" +2 −1\n"));
        assert!(text.contains("-old\n+new\n+extra\n"));
        assert!(text.contains("+created\n"));
        assert!(text.contains("-removed\n"));
    }
    #[test]
    fn preserves_text_and_signals_bounded_diff_truncation() {
        let fields = fields(serde_json::json!([
            {"type":"content","content":{"type":"text","text":"original output"}},
            {"type":"diff","path":"/home/cma/large.txt","newText":"line\n".repeat(10000)}
        ]));
        let values = crate::reported_tool_text(&fields);
        assert!(values[0].starts_with("File changes: 1 +10000 −0"));
        assert!(values[0].len() > LIMIT && values[0].len() < LIMIT + 20);
        assert_eq!(values[1], "original output");
    }
}

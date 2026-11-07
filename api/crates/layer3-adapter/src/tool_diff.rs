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
    reported_diffs_with_budget(content, || std::time::Duration::from_millis(50))
}

fn reported_diffs_with_budget(
    content: &[ToolCallContent],
    mut budget: impl FnMut() -> std::time::Duration,
) -> Option<String> {
    let diffs: Vec<_> = content
        .iter()
        .filter_map(|value| match value {
            ToolCallContent::Diff(diff) => Some(diff),
            _ => None,
        })
        .map(|value| {
            let deadline = std::time::Instant::now() + budget();
            let diff = TextDiff::configure()
                .deadline(deadline)
                .diff_lines(value.old_text.as_deref().unwrap_or(""), &value.new_text);
            let exact = std::time::Instant::now() < deadline;
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
                exact,
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
    if diffs.iter().all(|(_, _, _, _, exact)| *exact) {
        let added: u64 = diffs.iter().map(|(_, _, added, _, _)| added).sum();
        let removed: u64 = diffs.iter().map(|(_, _, _, removed, _)| removed).sum();
        let _ = writeln!(output, "File changes: {} +{added} −{removed}", diffs.len());
    } else {
        let _ = writeln!(output, "File changes: {} counts unavailable", diffs.len());
    }
    for (path, _, added, removed, exact) in &diffs {
        if *exact {
            let _ = writeln!(output, "File: {path} +{added} −{removed}");
        } else {
            let _ = writeln!(output, "File: {path} counts unavailable");
        }
    }
    for (path, diff, _, _, _) in &diffs {
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

#[cfg(test)]
mod budget_tests {
    use super::*;
    #[test]
    fn degraded_file_has_no_exact_totals_and_does_not_spend_the_next_files_budget() {
        let fields: crate::ToolCallUpdateFields =
            serde_json::from_value(serde_json::json!({"content":[
                {"type":"diff", "path":"first", "oldText":"a\nb\nc\n", "newText":"a\nx\nc\n"},
                {"type":"diff", "path":"second", "oldText":"before\n", "newText":"after\n"}
            ]}))
            .unwrap();
        let mut first = true;
        let text = reported_diffs_with_budget(fields.content.as_ref().unwrap(), || {
            if std::mem::take(&mut first) {
                std::time::Duration::ZERO
            } else {
                std::time::Duration::from_secs(1)
            }
        })
        .unwrap();
        assert!(text.starts_with("File changes: 2 counts unavailable\n"));
        assert!(text.contains("File: \"first\" counts unavailable\n"));
        assert!(text.contains("File: \"second\" +1 −1\n"));
        assert!(text.contains("-before\n+after\n"));
    }
}

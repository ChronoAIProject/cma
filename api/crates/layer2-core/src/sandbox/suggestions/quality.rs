use super::SuggestionContext;
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidates {
    suggestions: Vec<Candidate>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    text: String,
    evidence: String,
}

pub(super) fn candidates(output: &str, context: &SuggestionContext) -> Option<Vec<String>> {
    if output.len() > 4096 {
        return None;
    }
    let parsed: Candidates = serde_json::from_str(output).ok()?;
    if parsed.suggestions.len() > 3 {
        return None;
    }
    let mut seen = BTreeSet::new();
    Some(
        parsed
            .suggestions
            .into_iter()
            .filter_map(|candidate| {
                let text = candidate.text;
                let normalized = text.to_lowercase();
                let evidence = candidate.evidence;
                let grounded = evidence.chars().count() >= 4
                    && evidence.len() <= 240
                    && (context.goal.contains(&evidence) || context.answer.contains(&evidence));
                let generic = [
                    "continue",
                    "go on",
                    "tell me more",
                    "what next",
                    "继续",
                    "请继续",
                    "还有吗",
                ]
                .iter()
                .any(|phrase| normalized.trim_end_matches(['.', '?', '。', '？', '!']) == *phrase);
                let multiple = [';', '；', '\n']
                    .iter()
                    .any(|separator| text.contains(*separator))
                    || text.matches(['?', '？']).count() > 1
                    || [
                        " and then ",
                        " also ",
                        " as well as ",
                        "以及",
                        "并且",
                        "同时",
                        "然后",
                        "并",
                    ]
                    .iter()
                    .any(|join| normalized.contains(join))
                    || (normalized.contains(" and ") && !normalized.starts_with("compare "));
                let resource_claim = [
                    "://",
                    "/",
                    "\\",
                    ".md",
                    ".pdf",
                    ".doc",
                    ".xlsx",
                    ".csv",
                    ".txt",
                    ".png",
                    ".zip",
                    ".ppt",
                    ".js",
                    ".py",
                    ".rs",
                    ".json",
                    ".yaml",
                    "download",
                    "upload",
                    "publish",
                    "deploy",
                    "send the",
                    "open the file",
                    "saved file",
                    "created file",
                    "下载",
                    "上传",
                    "发布",
                    "部署",
                    "发送",
                    "打开文件",
                    "生成的文件",
                    "保存的文件",
                ]
                .iter()
                .any(|term| normalized.contains(term));
                let key: String = normalized
                    .chars()
                    .filter(|character| character.is_alphanumeric())
                    .collect();
                let unsupported_number = text
                    .split(|character: char| !character.is_ascii_digit())
                    .filter(|number| !number.is_empty())
                    .any(|number| {
                        !context.goal.contains(number) && !context.answer.contains(number)
                    });
                let personal_claim = [
                    "i am ",
                    "i have ",
                    "i can ",
                    "i never ",
                    "i already ",
                    "我从未",
                    "我已经",
                    "我会",
                    "我没有",
                    "我是",
                ]
                .iter()
                .any(|prefix| normalized.starts_with(prefix));
                (grounded
                    && !generic
                    && !multiple
                    && !resource_claim
                    && !unsupported_number
                    && !personal_claim
                    && text == text.trim()
                    && text.chars().count() >= 8
                    && text.chars().count() <= 180
                    && !text.chars().any(char::is_control)
                    && seen.insert(key))
                .then_some(text)
            })
            .collect(),
    )
}

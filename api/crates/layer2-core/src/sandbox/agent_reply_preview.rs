#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentReplyPreview {
    Ready {
        source_through_seq: u64,
        reply: AssistantReplyPreview,
    },
    None {
        source_through_seq: u64,
    },
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantReplyPreview {
    pub text: String,
    pub truncated: bool,
    pub message_sequence: u64,
    pub turn_sequence: u64,
    pub appended_at: String,
}

pub fn normalize_reply_preview(content: &str) -> Option<(String, bool)> {
    let mut text = String::new();
    let mut count = 0;
    let mut space = false;
    for c in content.chars() {
        if c.is_whitespace() {
            space = !text.is_empty();
            continue;
        }
        if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            continue;
        }
        if count + usize::from(space) >= 240 {
            return Some((text, true));
        }
        if space {
            text.push(' ');
            count += 1;
        }
        text.push(c);
        count += 1;
        space = false;
    }
    (!text.is_empty()).then_some((text, false))
}

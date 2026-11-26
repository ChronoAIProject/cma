use super::{
    text::{chunks, escaped, literal, TextStyle},
    RenderedPart, Renderer,
};
use crate::tools::Capabilities;
use cmaeg_core::message::{
    Block, BlockRendering, BlockRenderingOutcome, ChoiceOption, OutboundMessage, RenderingOutcome,
    RenderingReport,
};
/// Text renderer for NyxID relay platforms. Native controls are left to the
/// interactive path only when the capabilities advertise an inline keyboard;
/// otherwise choices degrade to numbered text.
pub struct RelayText;
impl RelayText {
    /// Ordinary NyxID replies preserve one logical message. NyxID owns the
    /// final platform split; native sends and interactive edits use `render`.
    pub fn whole(
        &self,
        message: &OutboundMessage,
        capabilities: &Capabilities,
    ) -> (RenderedPart, RenderingReport) {
        self.whole_with_commonmark(message, capabilities, false)
    }

    /// NyxID owns CommonMark parsing and platform splitting; keep source bytes
    /// intact so literals and formatting are not escaped a second time.
    pub fn whole_with_commonmark(
        &self,
        message: &OutboundMessage,
        capabilities: &Capabilities,
        commonmark: bool,
    ) -> (RenderedPart, RenderingReport) {
        let (text, report) = sections(message, capabilities);
        (
            RenderedPart {
                text: if commonmark { text } else { escaped(&text) },
            },
            report,
        )
    }
}
impl Renderer for RelayText {
    fn render(
        &self,
        message: &OutboundMessage,
        capabilities: &Capabilities,
    ) -> (Vec<RenderedPart>, RenderingReport) {
        render_parts(message, capabilities, TextStyle::Escaped)
    }
}

/// Text renderer for destinations that show plain text (Lark, Feishu) or
/// parse their own Markdown (Discord). Mention neutralization and
/// control/bidi filtering match `RelayText`; Markdown punctuation stays
/// literal so URLs, masked links and intentional backslashes survive.
pub struct LiteralText;
impl LiteralText {
    /// One logical message, for native edits that replace a whole message.
    pub fn whole(
        &self,
        message: &OutboundMessage,
        capabilities: &Capabilities,
    ) -> (RenderedPart, RenderingReport) {
        let (text, report) = sections(message, capabilities);
        (
            RenderedPart {
                text: literal(&text),
            },
            report,
        )
    }
}
impl Renderer for LiteralText {
    fn render(
        &self,
        message: &OutboundMessage,
        capabilities: &Capabilities,
    ) -> (Vec<RenderedPart>, RenderingReport) {
        render_parts(message, capabilities, TextStyle::Literal)
    }
}

fn render_parts(
    message: &OutboundMessage,
    capabilities: &Capabilities,
    style: TextStyle,
) -> (Vec<RenderedPart>, RenderingReport) {
    let (text, mut report) = sections(message, capabilities);
    let parts: Vec<_> = chunks(
        &text,
        (capabilities.max_text_chars as usize).clamp(2, 4000),
        style,
    )
    .into_iter()
    .map(|text| RenderedPart { text })
    .collect();
    if parts.len() > 1 {
        report.outcome = RenderingOutcome::Degraded;
        // The sentinel names a whole-message transformation, outside block indices 0..15.
        report.blocks.push(BlockRendering {
            index: u16::MAX,
            outcome: BlockRenderingOutcome::Degraded {
                how: format!("chunked:{}", parts.len()),
            },
        });
    }
    (parts, report)
}
fn sections(message: &OutboundMessage, capabilities: &Capabilities) -> (String, RenderingReport) {
    let mut sections = Vec::new();
    if let Some(text) = &message.text {
        sections.push(text.clone());
    }
    let mut report = RenderingReport {
        outcome: RenderingOutcome::Rendered,
        blocks: vec![],
    };
    for (index, block) in message.blocks.iter().enumerate() {
        let (text, degradation) = render_block(
            block,
            matches!(
                capabilities.buttons.as_str(),
                "inline_keyboard" | "native_card"
            ),
        );
        if !text.is_empty() {
            sections.push(text);
        }
        let outcome = match degradation {
            Some(how) => {
                report.outcome = RenderingOutcome::Degraded;
                BlockRenderingOutcome::Degraded { how: how.into() }
            }
            None => BlockRenderingOutcome::Exact,
        };
        report.blocks.push(BlockRendering {
            index: index as u16,
            outcome,
        });
    }
    (
        if sections.is_empty()
            && message.native_attachments.is_empty()
            && message.whatsapp_template.is_none()
        {
            "Choose an option".into()
        } else {
            sections.join("\n\n")
        },
        report,
    )
}
fn render_block(block: &Block, native: bool) -> (String, Option<&'static str>) {
    match block {
        Block::Text { text } => (text.clone(), None),
        Block::Card {
            title,
            body,
            fields,
            image,
        } => {
            let mut lines = vec![format!("*{title}*")];
            lines.extend(body.iter().cloned());
            lines.extend(
                fields
                    .iter()
                    .map(|field| format!("{}: {}", field.label, field.value)),
            );
            lines.extend(image.iter().cloned());
            (lines.join("\n"), Some("text"))
        }
        Block::Buttons { .. } if native => (String::new(), None),
        Block::Choice { prompt, .. } if native => (prompt.clone(), None),
        Block::Link { .. } if native => (String::new(), None),
        Block::Buttons { options, .. } => (
            format!("{}\nReply with a number.", numbered(options)),
            Some("numbered_text"),
        ),
        Block::Choice {
            prompt,
            options,
            multi,
            ..
        } => (
            format!(
                "{prompt}\n{}\n{}",
                numbered(options),
                if *multi {
                    "(pick several, e.g. 1,3)"
                } else {
                    "Reply with a number."
                }
            ),
            Some("numbered_text"),
        ),
        Block::Form { fields, .. } => (
            fields
                .iter()
                .map(|field| {
                    format!(
                        "{}:{}",
                        field.label,
                        if field.required { " (required)" } else { "" }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Some("plain_text_form"),
        ),
        Block::Attachment {
            reference, caption, ..
        } => {
            let filename = reference
                .split(['?', '#'])
                .next()
                .unwrap_or(reference)
                .rsplit('/')
                .find(|part| !part.is_empty())
                .unwrap_or("attachment");
            let mut lines: Vec<_> = caption.iter().cloned().collect();
            lines.push(format!("[attachment: {filename}]"));
            if url::Url::parse(reference).is_ok_and(|url| {
                url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
            }) {
                lines.push(reference.clone());
            }
            (lines.join("\n"), Some("link_text"))
        }
        Block::Link { url, label } => (
            label
                .as_ref()
                .map_or_else(|| url.clone(), |label| format!("{label} ({url})")),
            None,
        ),
    }
}
fn numbered(options: &[ChoiceOption]) -> String {
    options
        .iter()
        .enumerate()
        .map(|(i, option)| format!("{}) {}", i + 1, option.label))
        .collect::<Vec<_>>()
        .join("\n")
}

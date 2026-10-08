use super::{
    text::{chunks_reserving, escape},
    RenderedPart, Renderer,
};
use crate::tools::Capabilities;
use cmaeg_core::message::{
    Block, BlockRendering, BlockRenderingOutcome, ChoiceOption, OutboundMessage, RenderingOutcome,
    RenderingReport,
};
/// Text renderer for NyxID relay platforms and native Telegram. Native
/// controls are left to the interactive path only when the capabilities
/// advertise an inline keyboard; otherwise choices degrade to numbered text.
pub struct RelayText;
impl Renderer for RelayText {
    fn render(
        &self,
        message: &OutboundMessage,
        capabilities: &Capabilities,
    ) -> (Vec<RenderedPart>, RenderingReport) {
        self.render_labeled(message, capabilities, None)
    }
}
impl RelayText {
    /// Render `message`, opening its first part with `[<label>] ` when a
    /// label is given. The label is escaped as body text is, its prefix
    /// counts toward the first part's UTF-16 budget, and it is not a block,
    /// so the report's block indexes are those of the unlabeled message.
    /// Every chat transport renders through here; there is no second
    /// labeled renderer.
    pub fn render_labeled(
        &self,
        message: &OutboundMessage,
        capabilities: &Capabilities,
        label: Option<&str>,
    ) -> (Vec<RenderedPart>, RenderingReport) {
        let mut sections = Vec::new();
        if let Some(text) = &message.text {
            sections.push(text.clone());
        }
        let mut report = RenderingReport {
            outcome: RenderingOutcome::Rendered,
            blocks: vec![],
        };
        for (index, block) in message.blocks.iter().enumerate() {
            let (text, degradation) =
                render_block(block, capabilities.buttons == "inline_keyboard");
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
        let prefix = label.map(|label| format!("[{}] ", escape(label)));
        let reserved = prefix
            .as_ref()
            .map_or(0, |prefix| prefix.encode_utf16().count());
        let mut parts: Vec<_> = chunks_reserving(
            &if sections.is_empty() {
                "Choose an option".into()
            } else {
                sections.join("\n\n")
            },
            (capabilities.max_text_chars as usize).clamp(2, 4000),
            reserved,
        )
        .into_iter()
        .map(|text| RenderedPart { text })
        .collect();
        if let (Some(prefix), Some(first)) = (prefix, parts.first_mut()) {
            first.text.insert_str(0, &prefix);
        }
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

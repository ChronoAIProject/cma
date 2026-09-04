use layer3_adapter::wire::{ResponseFrame, WireFailure, WireOperation};

const MAX_DETAIL_CHARS: usize = 1_024;

#[derive(Debug)]
pub(crate) struct RuntimeError {
    pub code: &'static str,
    pub detail: String,
    pub retryable: bool,
}

impl RuntimeError {
    pub(crate) fn rejected(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: bounded(detail.into()),
            retryable: false,
        }
    }

    pub(crate) fn unavailable(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: bounded(detail.into()),
            retryable: true,
        }
    }

    pub(crate) fn response(self, operation: WireOperation) -> ResponseFrame {
        ResponseFrame::Failure(WireFailure {
            operation,
            code: self.code.into(),
            detail: self.detail,
            retryable: self.retryable,
        })
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for RuntimeError {}

pub(crate) type Result<T> = std::result::Result<T, RuntimeError>;

fn bounded(value: String) -> String {
    let mut out = String::with_capacity(value.len().min(MAX_DETAIL_CHARS));
    for character in value.chars() {
        if out.chars().count() >= MAX_DETAIL_CHARS {
            break;
        }
        out.push(
            if character.is_control() && !matches!(character, '\n' | '\t') {
                ' '
            } else {
                character
            },
        );
    }
    out
}

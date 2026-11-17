//! Closed response classifications; never infer a stage from untrusted error text.
use axum::response::Response;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureStage {
    RequestValidation,
    CreatorAuthentication,
    ChannelRecord,
    ChannelProvider,
    ChannelOperation,
    NativeLarkSource,
    NativeLarkAuthentication,
    NativeLarkReadStatus,
    NativeLarkControlAdmission,
    NativeLarkAdmission,
    IntakeDisabled,
    IngressCapacity,
    CallbackTimeout,
    Panic,
}

impl FailureStage {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RequestValidation => "request_validation",
            Self::CreatorAuthentication => "creator_authentication",
            Self::ChannelRecord => "channel_record",
            Self::ChannelProvider => "channel_provider",
            Self::ChannelOperation => "channel_operation",
            Self::NativeLarkSource => "native_lark_source",
            Self::NativeLarkAuthentication => "native_lark_authentication",
            Self::NativeLarkReadStatus => "native_lark_read_status",
            Self::NativeLarkControlAdmission => "native_lark_control_admission",
            Self::NativeLarkAdmission => "native_lark_admission",
            Self::IntakeDisabled => "intake_disabled",
            Self::IngressCapacity => "ingress_capacity",
            Self::CallbackTimeout => "callback_timeout",
            Self::Panic => "panic",
        }
    }

    pub(crate) fn response(self, mut response: Response) -> Response {
        response.extensions_mut().insert(self);
        response
    }
}

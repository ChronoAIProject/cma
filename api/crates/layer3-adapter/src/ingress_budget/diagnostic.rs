use std::{fmt, future::Future};

use super::{
    AdmissionRefusal, AdmittedError, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    IngressWorkScope, OwnedWork,
};
use crate::AdapterError;

struct Context {
    scope: IngressWorkScope,
    policy: IngressOwnershipPolicy,
    class: IngressClass,
}

tokio::task_local! {
    static CURRENT: Context;
}

impl<F, T> OwnedWork<F>
where
    F: Future<Output = crate::Result<T>>,
{
    /// Install diagnostic admission while polling this owned operation. Detached
    /// tasks must install their own context; task locals are not inherited.
    /// The work plan must cover its other allocations and returned payloads;
    /// wrapping an already allocated error does not establish pre-admission.
    pub async fn resolve_admitted_result(
        self,
        policy: &IngressOwnershipPolicy,
        class: IngressClass,
    ) -> Result<OwnedWork<T>, AdmittedError> {
        if !self.belongs_to(policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "diagnostic operation ownership",
            }
            .into());
        }
        let scope = self.work_scope().expect("validated operation owner");
        // The existing scope already owns its reservation Arc. The returned
        // error can add one payload Arc, including counters and alignment.
        scope.admit_extra(
            policy,
            class,
            AllocationPlan::checked(
                "runtime error owner",
                &[size_of::<AdapterError>() + 2 * size_of::<usize>() + align_of::<AdapterError>()],
            )?,
        )?;
        let context = Context {
            scope,
            policy: policy.clone(),
            class,
        };
        let result = CURRENT.scope(context, self.resolve()).await;
        match result.transpose() {
            Ok(value) => Ok(value),
            Err(error) => Err(error.into_shared_admitted()?.into()),
        }
    }
}

/// Call only with borrowed built-in/JSON values: arbitrary Display callbacks can
/// allocate during the counting pass. This preserves each legacy error category.
pub(crate) fn make(
    category: fn(String) -> AdapterError,
    detail: fmt::Arguments<'_>,
) -> AdapterError {
    match format_detail(detail) {
        Ok(detail) => category(detail),
        Err(AdmissionRefusal::AtCapacity { .. }) => AdapterError::AtCapacity(String::new()),
        Err(AdmissionRefusal::InvalidPlan { .. }) => AdapterError::Rejected(String::new()),
    }
}

/// An established outcome (including uncertain writes) cannot become a different
/// category because its diagnostic was refused. Never fall back to remote text
/// at a redaction boundary; an empty diagnostic preserves only the outcome.
pub(crate) fn make_preserving_category(
    category: fn(String) -> AdapterError,
    detail: fmt::Arguments<'_>,
) -> AdapterError {
    category(format_detail(detail).unwrap_or_default())
}

/// Fixed runtime failures keep their category even when text cannot be admitted.
pub(crate) fn text(detail: &'static str) -> String {
    match CURRENT.try_with(|context| format_admitted(context, format_args!("{detail}"))) {
        Ok(result) => result.unwrap_or_default(),
        Err(_) => detail.to_owned(),
    }
}

pub(crate) fn context(
    category: fn(String) -> AdapterError,
    message: String,
    context: fmt::Arguments<'_>,
) -> AdapterError {
    // The original text stays alive during formatting, so both allocations
    // retain their allowances. A refused addition leaves the original intact.
    category(format_detail(format_args!("{context}: {message}")).unwrap_or(message))
}

fn format_detail(detail: fmt::Arguments<'_>) -> Result<String, AdmissionRefusal> {
    CURRENT
        .try_with(|context| format_admitted(context, detail))
        .unwrap_or_else(|_| Ok(fmt::format(detail)))
}

fn format_admitted(
    context: &Context,
    detail: fmt::Arguments<'_>,
) -> Result<String, AdmissionRefusal> {
    let invalid = || AdmissionRefusal::InvalidPlan {
        phase: "runtime diagnostic formatting",
    };
    let mut length = Length(0);
    fmt::write(&mut length, detail).map_err(|_| invalid())?;
    context.scope.admit_extra(
        &context.policy,
        context.class,
        AllocationPlan::checked("runtime diagnostic formatting", &[length.0])?,
    )?;
    let mut output = FixedText(String::with_capacity(length.0));
    fmt::write(&mut output, detail).map_err(|_| invalid())?;
    Ok(output.0)
}

struct Length(usize);

impl fmt::Write for Length {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.0 = self.0.checked_add(text.len()).ok_or(fmt::Error)?;
        Ok(())
    }
}

struct FixedText(String);

impl fmt::Write for FixedText {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if text.len() > self.0.capacity() - self.0.len() {
            return Err(fmt::Error);
        }
        self.0.push_str(text);
        Ok(())
    }
}

#[cfg(test)]
#[path = "diagnostic_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "diagnostic_context_tests.rs"]
mod context_tests;

//! Per-write spans observe existing adapter results; they never retry a write.
use crate::telemetry::turn_trace::{self, BoundOperation, Outcome, Stage};
use std::future::Future;

pub(in crate::delivery) fn part<T>(
    index: usize,
    upload: bool,
    future: impl Future<Output = T>,
    classify: fn(&T) -> Outcome,
) -> impl Future<Output = T> {
    if !turn_trace::enabled() {
        return futures_util::future::Either::Right(future);
    }
    let stage = if upload {
        Stage::ChannelPart
    } else {
        Stage::Platform
    };
    let Some(trace) = turn_trace::start_part(stage, u32::try_from(index).ok())
        .map(BoundOperation::from_operation)
    else {
        return futures_util::future::Either::Right(future);
    };
    let future = Box::pin(future);
    let classify = move |result: &T| match classify(result) {
        // Upload acceptance proves no recipient delivery.
        Outcome::Confirmed if upload => Outcome::Completed,
        outcome => outcome,
    };
    futures_util::future::Either::Left(async move {
        let result = super::observed(Some(trace), future, &classify).await;
        super::parts::record(classify(&result));
        result
    })
}

pub(in crate::delivery) fn telegram<T>(result: &Result<T, crate::telegram::Error>) -> Outcome {
    use crate::telegram::Error;
    match result {
        Ok(_) => Outcome::Confirmed,
        Err(Error::Unconfirmed) => Outcome::Unknown,
        Err(Error::Rejected | Error::RateLimited(_)) => Outcome::Rejected,
    }
}

pub(in crate::delivery) fn discord<T>(
    result: &Result<T, crate::discord::rest::RestError>,
) -> Outcome {
    use crate::discord::rest::RestError;
    match result {
        Ok(_) => Outcome::Confirmed,
        Err(RestError::Unavailable | RestError::Unknown) => Outcome::Unknown,
        Err(RestError::Rejected | RestError::NotFound | RestError::RateLimited(_)) => {
            Outcome::Rejected
        }
    }
}

pub(in crate::delivery) fn whatsapp<T>(
    result: &Result<Option<T>, crate::whatsapp::Error>,
) -> Outcome {
    match result {
        Ok(None) => Outcome::Completed,
        _ => whatsapp_write(result),
    }
}

pub(in crate::delivery) fn whatsapp_write<T>(
    result: &Result<T, crate::whatsapp::Error>,
) -> Outcome {
    use crate::whatsapp::Error;
    match result {
        Ok(_) => Outcome::Confirmed,
        Err(Error::Unconfirmed) => Outcome::Unknown,
        Err(Error::Rejected | Error::Invalid | Error::CustodyUnproved | Error::RateLimited) => {
            Outcome::Rejected
        }
    }
}

pub(in crate::delivery) fn lark<T>(
    result: &Result<Option<T>, crate::native_lark::Error>,
) -> Outcome {
    match result {
        Ok(None) => Outcome::Completed,
        _ => lark_write(result),
    }
}

pub(in crate::delivery) fn lark_write<T>(result: &Result<T, crate::native_lark::Error>) -> Outcome {
    use crate::native_lark::Error;
    match result {
        Ok(_) => Outcome::Confirmed,
        Err(Error::Unconfirmed) => Outcome::Unknown,
        Err(Error::Rejected | Error::Invalid | Error::RateLimited) => Outcome::Rejected,
    }
}

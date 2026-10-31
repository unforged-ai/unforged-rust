use std::fmt;
use std::time::Duration;

use serde_json::Value;

/// What kind of error answer the API gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ApiErrorKind {
    /// The request or one of its files can't be used (400, 413, 415, 416 or 422).
    InvalidRequest,
    /// The API key is missing, wrong or revoked (401).
    Authentication,
    /// This needs active billing on the organisation (402).
    BillingRequired,
    /// The key doesn't have the permission this call needs (403).
    PermissionDenied,
    /// No such check, subject, reference or record (404).
    NotFound,
    /// The request clashes with how things stand now (409).
    Conflict,
    /// Opening this record needs a reason (428).
    ReasonRequired,
    /// The organisation lives in another region (421).
    WrongRegion,
    /// Too many requests, or the month's quota is used up (429).
    RateLimited,
    /// Something failed on our side; safe to retry later (5xx).
    Server,
    /// Any other error answer.
    Other,
}

impl ApiErrorKind {
    fn for_answer(status: Option<u16>, code: Option<&str>) -> Self {
        if matches!(code, Some("rate_limited" | "quota_exceeded")) {
            return Self::RateLimited;
        }
        match status {
            Some(s) if s >= 500 => Self::Server,
            Some(400 | 413 | 415 | 416 | 422) => Self::InvalidRequest,
            Some(401) => Self::Authentication,
            Some(402) => Self::BillingRequired,
            Some(403) => Self::PermissionDenied,
            Some(404) => Self::NotFound,
            Some(409) => Self::Conflict,
            Some(421) => Self::WrongRegion,
            Some(428) => Self::ReasonRequired,
            Some(429) => Self::RateLimited,
            _ => Self::Other,
        }
    }
}

/// An error answer: `code` is the stable error code and `detail` says what went wrong.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub kind: ApiErrorKind,
    pub status: Option<u16>,
    pub code: Option<String>,
    pub detail: String,
    pub request_id: Option<String>,
    /// The whole problem document.
    pub problem: Value,
    /// How long the API asked us to wait before trying again.
    pub retry_after: Option<Duration>,
}

impl ApiError {
    pub(crate) fn new(
        status: Option<u16>,
        problem: Value,
        request_id: Option<String>,
        retry_after: Option<Duration>,
    ) -> Self {
        let code = problem
            .get("code")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let detail = problem
            .get("detail")
            .or_else(|| problem.get("title"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| status.map_or_else(|| "error".into(), |s| format!("HTTP {s}")));
        let request_id = problem
            .get("request_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or(request_id);
        Self {
            kind: ApiErrorKind::for_answer(status, code.as_deref()),
            status,
            code,
            detail,
            request_id,
            problem,
            retry_after,
        }
    }

    /// An error with no answer behind it, such as a subject with no references offline.
    pub(crate) fn local(kind: ApiErrorKind, code: &str, detail: impl Into<String>) -> Self {
        Self {
            kind,
            status: None,
            code: Some(code.into()),
            detail: detail.into(),
            request_id: None,
            problem: Value::Null,
            retry_after: None,
        }
    }

    /// The host to call instead, on a wrong-region answer.
    pub fn api_host(&self) -> Option<&str> {
        self.problem.get("api_host").and_then(Value::as_str)
    }

    /// The job the answer is about, when it names one.
    pub fn job_id(&self) -> Option<&str> {
        self.problem.get("job_id").and_then(Value::as_str)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)?;
        if let Some(c) = &self.code {
            write!(f, " [{c}]")?;
        }
        if let Some(r) = &self.request_id {
            write!(f, " (request {r})")?;
        }
        Ok(())
    }
}

/// Every error the SDK returns.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// An error answer from the API (or its offline equivalent, such as no references).
    Api(Box<ApiError>),
    /// The call isn't available in this mode; the message says what to do instead.
    NotSupported(String),
    /// An argument can't be used; the message says why.
    InvalidInput(String),
    /// The document couldn't be checked; `kind` and `detail` say why.
    CheckFailed {
        kind: String,
        detail: String,
        job_id: Option<String>,
    },
    /// A check didn't finish within the time allowed. It carries on: `job_id` finds it later.
    CheckTimeout {
        message: String,
        job_id: Option<String>,
    },
    /// The API couldn't be reached. `sent` is false when the request certainly never left.
    Connection { message: String, sent: bool },
    /// The offline binary failed.
    Offline {
        message: String,
        exit_code: Option<i32>,
        stderr: String,
    },
    /// The offline licence is missing, invalid or expired.
    Licence { message: String },
    /// The offline licence doesn't include this command.
    NotLicensed { message: String },
    /// A downloaded record didn't match its receipt, so it wasn't kept.
    Integrity { message: String, record_id: String },
    /// A webhook's signature didn't verify, or its timestamp is too old.
    WebhookSignature(String),
    /// An answer couldn't be read.
    Decode(String),
    /// A local file couldn't be read or written.
    Io(std::io::Error),
}

impl Error {
    /// The stable error code, when there is one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Error::Api(e) => e.code.as_deref(),
            Error::CheckFailed { kind, .. } => Some(kind),
            _ => None,
        }
    }

    /// The API error, when this is one.
    pub fn api(&self) -> Option<&ApiError> {
        match self {
            Error::Api(e) => Some(e),
            _ => None,
        }
    }

    /// No such check, subject, reference or record, online or offline.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Error::Api(e) if e.kind == ApiErrorKind::NotFound)
    }

    pub(crate) fn not_supported(message: impl Into<String>) -> Self {
        Error::NotSupported(message.into())
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Error::InvalidInput(message.into())
    }

    pub(crate) fn no_references(detail: impl Into<String>) -> Self {
        Error::Api(Box::new(ApiError::local(
            ApiErrorKind::NotFound,
            "no_usable_references",
            detail,
        )))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Api(e) => e.fmt(f),
            Error::NotSupported(m)
            | Error::InvalidInput(m)
            | Error::WebhookSignature(m)
            | Error::Decode(m) => f.write_str(m),
            Error::CheckFailed { kind, detail, .. } => write!(f, "{detail} [{kind}]"),
            Error::CheckTimeout { message, .. }
            | Error::Connection { message, .. }
            | Error::Offline { message, .. }
            | Error::Integrity { message, .. }
            | Error::Licence { message }
            | Error::NotLicensed { message } => f.write_str(message),
            Error::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// The SDK's result type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

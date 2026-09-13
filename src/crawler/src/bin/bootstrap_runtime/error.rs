use std::{error::Error, fmt};

/// Fixed CLI taxonomy. Raw causes are retained privately, never exposed through formatting
/// or source traversal (SQLx migration errors can contain SQL, paths and server payloads).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Code {
    Usage,
    Config,
    UnsupportedStage,
    UnsupportedPlatform,
    NonLocalEndpoint,
    NotFresh,
    Prerequisite,
    UnsupportedSource,
    Dependency,
    Verification,
    Deadline,
    Cleanup,
    UnknownOutcome,
    Runtime,
    Output,
    Legacy,
}

impl Code {
    pub(super) const fn text(self) -> &'static str {
        match self {
            Self::Usage => "USAGE_ERROR",
            Self::Config => "CONFIG_ERROR",
            Self::UnsupportedStage => "UNSUPPORTED_STAGE",
            Self::UnsupportedPlatform => "UNSUPPORTED_PLATFORM",
            Self::NonLocalEndpoint => "NONLOCAL_ENDPOINT",
            Self::NotFresh => "NOT_FRESH",
            Self::Prerequisite => "PREREQUISITE_MISSING",
            Self::UnsupportedSource => "UNSUPPORTED_SOURCE",
            Self::Dependency => "DEPENDENCY_FAILED",
            Self::Verification => "VERIFICATION_FAILED",
            Self::Deadline => "DEADLINE_EXCEEDED",
            Self::Cleanup => "CLEANUP_UNCONFIRMED",
            Self::UnknownOutcome => "UNKNOWN_OUTCOME",
            Self::Runtime => "RUNTIME_FAILED",
            Self::Output => "OUTPUT_FAILED",
            Self::Legacy => "LEGACY_FAILED",
        }
    }

    pub(super) const fn exit(self) -> u8 {
        match self {
            Self::Usage => 2,
            Self::Config
            | Self::UnsupportedStage
            | Self::UnsupportedPlatform
            | Self::NonLocalEndpoint => 3,
            Self::NotFresh | Self::Prerequisite | Self::UnsupportedSource => 4,
            Self::Dependency | Self::Verification => 5,
            Self::Deadline | Self::Cleanup => 6,
            Self::UnknownOutcome => 7,
            Self::Runtime | Self::Output | Self::Legacy => 8,
        }
    }
}

pub(super) struct Failure {
    pub(super) code: Code,
    cause: Option<RedactedCause>,
}

struct RedactedCause {
    _original: Box<dyn Error + Send + Sync>,
}

impl Failure {
    pub(super) fn new(code: Code) -> Self {
        Self { code, cause: None }
    }

    pub(super) fn caused(code: Code, cause: impl Error + Send + Sync + 'static) -> Self {
        Self {
            code,
            cause: Some(RedactedCause {
                _original: Box::new(cause),
            }),
        }
    }

    pub(super) fn after_writes(self, possible: bool) -> Self {
        if possible && self.code != Code::UnknownOutcome {
            Self::caused(Code::UnknownOutcome, self)
        } else {
            self
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code.text())
    }
}
impl fmt::Debug for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl Error for Failure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause.as_ref().map(|cause| cause as &dyn Error)
    }
}
impl fmt::Display for RedactedCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cause withheld")
    }
}
impl fmt::Debug for RedactedCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl Error for RedactedCause {}

impl From<platform_postgres::PostgresPoolConfigError> for Failure {
    fn from(error: platform_postgres::PostgresPoolConfigError) -> Self {
        Self::caused(Code::Config, error)
    }
}
impl From<sqlx::Error> for Failure {
    fn from(error: sqlx::Error) -> Self {
        Self::caused(Code::Dependency, error)
    }
}

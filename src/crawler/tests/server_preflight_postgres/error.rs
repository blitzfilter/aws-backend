use std::{error::Error, fmt, io};

pub(super) type TestResult<T = ()> = Result<T, TestError>;
type Cause = Box<dyn Error + Send + Sync>;

/// Test-provider causes remain owned but never escape formatting or source traversal.
pub(super) struct TestError {
    kind: &'static str,
    causes: Vec<Cause>,
}

impl TestError {
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn failure(kind: &'static str) -> Self {
        Self {
            kind,
            causes: Vec::new(),
        }
    }

    pub fn caused(kind: &'static str, cause: impl Into<Cause>) -> Self {
        Self {
            kind,
            causes: vec![cause.into()],
        }
    }

    pub fn context(self, kind: &'static str) -> Self {
        Self::caused(kind, self)
    }

    pub fn failures(kind: &'static str, failures: Vec<Self>) -> Self {
        Self {
            kind,
            causes: failures
                .into_iter()
                .map(|error| Box::new(error) as Cause)
                .collect(),
        }
    }

    /// Reconstruct only test-owned classifications, never sanitize or relay provider text.
    pub fn report_kind(value: &str) -> &'static str {
        const KINDS: &[&str] = &[
            "IO",
            "POSTGRES",
            "BASELINE_FIXTURE",
            "POSTGRES_CONFIG",
            "POSTGRES_CONNECT",
            "JSON",
            "TIMEOUT",
            "ENVIRONMENT",
            "OUTPUT_ENCODING",
            "FIXTURE_RECORD",
            "FIXTURE",
            "CHILD_START",
            "READER_START",
            "CHILD_REAP",
            "CHILD_DEADLINE",
            "CHILD_TERM",
            "CHILD_KILL",
            "CHILD_REAP_TIMEOUT",
            "READER_PANIC",
            "READER_JOIN_TIMEOUT",
            "PIPE_NONBLOCKING",
            "PIPE_EOF_TIMEOUT",
            "PIPE_READ",
            "IDLE_AUTH_DEADLINE",
            "IDLE_AUTH_JOIN",
            "IDLE_AUTH_REQUEST",
            "IDLE_EARLY_EXIT",
            "IDLE_CONFIG_ORDER",
            "IDLE_CRAWLER_HISTORY_ORDER",
            "IDLE_BUSINESS_HISTORY_ORDER",
            "IDLE_LLM_ORDER",
            "IDLE_DEPENDENCIES_ORDER",
            "IDLE_NETWORK_ACTIVITY",
            "CASE_ASSERTION",
            "CASE_DEADLINE",
            "DATABASE_CLEANUP",
            "OBSERVER_CLOSE",
            "DATABASE_DROP",
            "ROLE_DROP",
            "DATABASE_ROLE_PRESENT",
        ];
        KINDS
            .iter()
            .copied()
            .find(|kind| *kind == value)
            .unwrap_or("UNRECOGNIZED_CLASSIFICATION")
    }
}

impl fmt::Display for TestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fixture error: {} (details redacted)", self.kind)
    }
}
impl fmt::Debug for TestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl Error for TestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        // Keep the original for inspection inside this test boundary only.
        let _retained = &self.causes;
        None
    }
}

macro_rules! private_causes {
    ($($ty:ty => $kind:literal),+ $(,)?) => {$(
        impl From<$ty> for TestError {
            fn from(error: $ty) -> Self { Self::caused($kind, error) }
        }
    )+};
}
private_causes! {
    io::Error => "IO",
    sqlx::Error => "POSTGRES",
    sqlx::migrate::MigrateError => "BASELINE_FIXTURE",
    platform_postgres::PostgresPoolConfigError => "POSTGRES_CONFIG",
    platform_postgres::PostgresConnectError => "POSTGRES_CONNECT",
    serde_json::Error => "JSON",
    tokio::time::error::Elapsed => "TIMEOUT",
    std::env::VarError => "ENVIRONMENT",
    std::str::Utf8Error => "OUTPUT_ENCODING",
    std::string::FromUtf8Error => "OUTPUT_ENCODING",
    std::num::ParseIntError => "FIXTURE_RECORD",
}
impl From<&'static str> for TestError {
    fn from(message: &'static str) -> Self {
        Self::caused("FIXTURE", io::Error::other(message))
    }
}
impl From<String> for TestError {
    fn from(message: String) -> Self {
        Self::caused("FIXTURE", io::Error::other(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_retain_provider_causes_without_exposing_any_error_format() -> TestResult {
        let error = TestError::from(sqlx::Error::Protocol(
            "unrecognized-private-provider-body".into(),
        ));
        let original = error.causes.first().ok_or("cause was discarded")?;
        assert!(original.is::<sqlx::Error>());
        assert!(error.source().is_none());
        assert!(
            format!("{error} {error:?} {error:#?}")
                .matches("details redacted")
                .count()
                == 3
        );
        assert!(
            !format!("{error} {error:?} {error:#?}").contains("unrecognized-private-provider-body")
        );
        let combined = TestError::failures("DATABASE_CLEANUP", vec![error]);
        assert!(combined.causes.len() == 1 && combined.source().is_none());
        assert!(
            !format!("{combined} {combined:?} {combined:#?}")
                .contains("unrecognized-private-provider-body")
        );
        let reports = crate::support::fixture_reports(
            "FAIL Valid: POSTGRES\nFAIL CatalogSearchPath: unrecognized-private-provider-body\nprovider arbitrary secret\n",
        );
        let pass = crate::Case::Valid.pass_report();
        let harness_output = format!("test {} ... \n{pass}\n", crate::support::CHILD_ENTRY);
        assert!(crate::support::fixture_reports(&harness_output)[0] == pass);
        assert!(reports.len() == 28);
        assert!(reports[0] == "FAIL Valid: POSTGRES");
        assert!(reports[1] == "FAIL CatalogSearchPath: UNRECOGNIZED_CLASSIFICATION");
        assert!(
            reports
                .iter()
                .all(|line| !line.contains("unrecognized-private-provider-body")
                    && !line.contains("provider arbitrary secret"))
        );
        Ok(())
    }
}

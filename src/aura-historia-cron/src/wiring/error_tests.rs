use super::*;
use std::io;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

pub(super) fn assert_redacted_chain(error: &(dyn Error + 'static)) {
    let mut current = Some(error);
    let mut depth = 0;
    while let Some(error) = current {
        let rendered = format!("{error} {error:#} {error:?} {error:#?}");
        for canary in [
            "value_canary",
            "database_canary",
            "username_canary",
            "password_canary",
            "project_canary",
            "location_canary",
            "model_canary",
            "deep_canary",
        ] {
            assert!(
                !rendered.contains(canary),
                "error chain leaked a canary at depth {depth}"
            );
        }
        depth += 1;
        assert!(depth < 16, "error source chain did not terminate");
        current = error.source();
    }
}

pub(super) fn original<E: Error + 'static>(error: &WiringError) -> TestResult<&E> {
    let wrapper = error
        .source()
        .and_then(|source| source.downcast_ref::<WiringErrorCause>())
        .ok_or("opaque wiring source missing")?;
    assert!(
        wrapper.source().is_none(),
        "opaque source exposed original chain"
    );
    wrapper
        ._original
        .downcast_ref::<E>()
        .ok_or("typed original cause lost".into())
}

#[derive(Debug, thiserror::Error)]
#[error("value_canary")]
struct CanaryError {
    #[source]
    source: io::Error,
}

fn canary() -> CanaryError {
    CanaryError {
        source: io::Error::other("deep_canary"),
    }
}

#[test]
fn should_redact_every_opaque_variant_and_retain_nested_typed_causes() -> TestResult {
    let errors = [
        WiringError::InvalidEnvEncoding {
            name: "STAGE",
            source: WiringErrorCause::new(canary()),
        },
        WiringError::InvalidNumber {
            name: "PERIODIC_MATCH_MAX_ATTEMPTS",
            source: WiringErrorCause::new(canary()),
        },
        WiringError::InvalidSchedule {
            source: WiringErrorCause::new(canary()),
        },
        WiringError::OpenSearch {
            source: WiringErrorCause::new(canary()),
        },
        WiringError::VertexCredentials {
            source: WiringErrorCause::new(canary()),
        },
        WiringError::VertexClient(WiringErrorCause::new(canary())),
        WiringError::Handler(WiringErrorCause::new(canary())),
    ];
    for error in errors {
        assert_redacted_chain(&error);
        let original = original::<CanaryError>(&error)?;
        assert_eq!(original.source.to_string(), "deep_canary");
        assert!(original.source().is_some());
    }
    Ok(())
}

#[test]
fn should_retain_cron_parser_errors_without_exposing_the_expression() -> TestResult {
    let error = validate_schedule("value_canary")
        .err()
        .ok_or("bad schedule accepted")?;
    assert_redacted_chain(&error);
    assert!(matches!(
        original::<cron_tab::CronError>(&error)?,
        cron_tab::CronError::ParseError(_)
    ));
    Ok(())
}

#[test]
fn should_retain_numeric_parser_errors_without_exposing_input() -> TestResult {
    let error = parse_number::<u64>("PERIODIC_MATCH_MAX_ATTEMPTS", "value_canary")
        .err()
        .ok_or("bad number accepted")?;
    assert_redacted_chain(&error);
    original::<std::num::ParseIntError>(&error)?;
    Ok(())
}

#[test]
fn should_retain_real_opensearch_build_errors_privately() -> TestResult {
    let source = opensearch::http::transport::BuildError::from(io::Error::other(canary()));
    assert!(format!("{source} {source:?}").contains("value_canary"));
    let error = WiringError::OpenSearch {
        source: WiringErrorCause::new(source),
    };
    assert_redacted_chain(&error);
    let original = original::<opensearch::http::transport::BuildError>(&error)?;
    assert!(matches!(
        original,
        opensearch::http::transport::BuildError::Io(_)
    ));
    Ok(())
}

#[test]
fn should_retain_real_google_credentials_build_errors_privately() -> TestResult {
    // A malformed explicit JSON value fails before ADC lookup or token-cache construction.
    let source =
        google_cloud_auth::credentials::service_account::Builder::new(r#""value_canary""#.parse()?)
            .build_access_token_credentials()
            .err()
            .ok_or("bad credential specification accepted")?;
    assert!(format!("{source} {source:?}").contains("value_canary"));
    let error = WiringError::VertexCredentials {
        source: WiringErrorCause::new(source),
    };
    assert_redacted_chain(&error);
    assert!(original::<google_cloud_auth::build_errors::Error>(&error)?.is_parsing());
    Ok(())
}

#[test]
fn should_retain_real_http_client_errors_without_exposing_urls() -> TestResult {
    let source = reqwest::Client::builder()
        .no_proxy()
        .build()?
        .get("http://[")
        .build()
        .err()
        .ok_or("bad request URL accepted")?
        .with_url(url::Url::parse(
            "https://username_canary:password_canary@example.test/value_canary",
        )?);
    assert!(format!("{source} {source:?}").contains("password_canary"));
    let error = WiringError::VertexClient(WiringErrorCause::new(source));
    assert_redacted_chain(&error);
    assert!(original::<reqwest::Error>(&error)?.url().is_some());
    Ok(())
}

#[test]
fn should_retain_service_error_and_its_raw_cause_only_privately() -> TestResult {
    use search_filter_service::use_cases::RunPeriodicSearchFilterMatchingError;

    let source = RunPeriodicSearchFilterMatchingError::RunLockFailed {
        source: Box::new(canary()),
    };
    let error = WiringError::Handler(WiringErrorCause::new(source));
    assert_redacted_chain(&error);
    let source = original::<RunPeriodicSearchFilterMatchingError>(&error)?;
    assert!(
        source
            .source()
            .is_some_and(|source| source.is::<CanaryError>())
    );
    Ok(())
}

#[test]
fn should_keep_shared_postgres_connection_and_schema_causes_redacted() -> TestResult {
    fn rejected_options<T: FromStr>(_: T) -> TestResult<T::Err> {
        "postgres://username_canary:password_canary@localhost/database_canary?sslmode=value_canary"
            .parse::<T>()
            .err()
            .ok_or("bad PostgreSQL options accepted".into())
    }

    let config = postgres_config(&mut |name| {
        match name {
            "STAGE" => Some("test"),
            "POSTGRES_SSL_MODE" => Some("disable"),
            "POSTGRES_HOST" => Some("localhost"),
            "POSTGRES_DATABASE" => Some("database_canary"),
            "POSTGRES_USERNAME" => Some("username_canary"),
            "POSTGRES_PASSWORD" => Some("password_canary"),
            _ => None,
        }
        .map(str::to_owned)
    })?;
    let source = rejected_options(config.connect_options())?;
    assert!(format!("{source} {source:?}").contains("value_canary"));
    let connection = WiringError::Postgres(source.into());
    let schema = WiringError::Schema(rejected_options(config.connect_options())?.into());
    assert!(
        connection
            .source()
            .is_some_and(|source| source.is::<PostgresConnectError>())
    );
    assert!(
        schema
            .source()
            .is_some_and(|source| source.is::<platform_postgres::PostgresSchemaError>())
    );
    for error in [connection, schema] {
        assert_redacted_chain(&error);
        assert!(error.source().and_then(Error::source).is_some());
    }
    Ok(())
}

#[test]
fn should_keep_typed_safe_url_and_postgres_config_errors() -> TestResult {
    let source = url::Url::parse("https://username_canary:password_canary@[")
        .err()
        .ok_or("bad endpoint accepted")?;
    let error = WiringError::OpenSearchUrl(source);
    assert_redacted_chain(&error);
    assert!(
        error
            .source()
            .is_some_and(|source| source.is::<url::ParseError>())
    );
    for error in [
        WiringError::CompositionSkipped,
        WiringError::InvalidPolicy,
        WiringError::MissingEnv {
            name: "VERTEX_AI_MODEL",
        },
        WiringError::PostgresConfig(PostgresPoolConfigError::InvalidInput("POSTGRES_PASSWORD")),
    ] {
        assert_redacted_chain(&error);
    }
    Ok(())
}

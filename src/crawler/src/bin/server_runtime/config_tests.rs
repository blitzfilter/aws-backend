use super::*;
use rstest::rstest;
use std::error::Error;

const SHA: &str = "c7a46b9b434eb0dc02c26a4e91edc863cae34896";
const CANARY: &str = "private-secret-canary";

fn environment() -> BTreeMap<&'static str, String> {
    [
        (
            "LOCAL_DB_URL",
            "postgres://fixture:private-secret-canary@127.0.0.1:1/crawler",
        ),
        (
            "BUSINESS_DATABASE_URL",
            "postgres://fixture:private-secret-canary@127.0.0.1:2/business",
        ),
        ("STAGE", "test"),
        ("POSTGRES_SSL_MODE", "disable"),
        ("SPIDER_MAX_SIZE_BYTES", "8388608"),
        ("VERTEX_AI_PROJECT_ID", "fixture-project"),
        ("VERTEX_AI_LOCATION", "global"),
    ]
    .into_iter()
    .map(|(key, value)| (key, value.to_owned()))
    .collect()
}

fn parse(env: &BTreeMap<&'static str, String>) -> Result<ServerConfig, ConfigError> {
    ServerConfig::from_lookup(Some(SHA), |key| {
        env.get(key).cloned().ok_or(VarError::NotPresent)
    })
}

fn assert_redacted(error: &dyn Error) {
    let mut current = Some(error);
    while let Some(error) = current {
        assert!(!format!("{error} {error:?} {error:#?}").contains(CANARY));
        current = error.source();
    }
}

#[test]
fn should_accept_only_exact_cli_modes() {
    for (args, mode) in [
        (vec![], Mode::Daemon),
        (vec!["--check-config"], Mode::CheckConfig),
        (vec!["--help"], Mode::Help),
    ] {
        assert_eq!(Mode::parse(args.into_iter().map(OsString::from)), Ok(mode));
    }
    for args in [
        vec![""],
        vec!["--"],
        vec!["-h"],
        vec!["--check-config=true"],
        vec!["--help", "--check-config"],
        vec!["--check-config", "--check-config"],
        vec![CANARY],
        vec!["--help", CANARY],
    ] {
        let result = Mode::parse(args.into_iter().map(OsString::from));
        assert_eq!(result, Err(ArgumentError::Invalid));
        assert!(!format!("{result:?}").contains(CANARY));
    }
}

#[test]
fn should_preserve_valid_cloudwatch_stream_names() -> Result<(), ConfigError> {
    for (value, expected) in [
        ("worker 1".to_owned(), "worker 1".to_owned()),
        ("worker-ä".to_owned(), "worker-ä".to_owned()),
        (" worker 1 ".to_owned(), "worker 1".to_owned()),
        ("ä".repeat(512), "ä".repeat(512)),
    ] {
        let mut env = environment();
        env.insert("CRAWLER_CLOUDWATCH_LOG_GROUP", "fixture-group".into());
        env.insert("CRAWLER_CLOUDWATCH_LOG_STREAM", value);
        assert_eq!(
            parse(&env)?.cloudwatch.map(|value| value.log_stream_name),
            Some(expected)
        );
    }
    let mut env = environment();
    env.insert("CRAWLER_CLOUDWATCH_LOG_STREAM", "ä".repeat(513));
    assert!(parse(&env).is_err());
    Ok(())
}

#[test]
fn should_require_canonical_build_sha_without_runtime_override() {
    assert!(matches!(
        validate_commit_sha(None),
        Err(ConfigError::Missing(_))
    ));
    for value in ["", " "] {
        assert!(matches!(
            validate_commit_sha(Some(value)),
            Err(ConfigError::Empty(_))
        ));
    }
    for value in [
        CANARY.to_owned(),
        SHA[..39].to_owned(),
        format!("{SHA}0"),
        SHA.to_uppercase(),
        format!(" {SHA}"),
    ] {
        let result = validate_commit_sha(Some(&value));
        assert!(matches!(result, Err(ConfigError::Malformed { .. })));
        if let Err(error) = result {
            assert_redacted(&error);
        }
    }
    assert!(matches!(validate_commit_sha(Some(SHA)), Ok(value) if value == SHA));
}

#[test]
fn should_preserve_defaults_and_redact_configuration() -> Result<(), ConfigError> {
    let config = parse(&environment())?;
    assert_eq!(config.commit_sha, SHA);
    assert_eq!(config.lifecycle.shutdown_grace, Duration::from_secs(300));
    assert_eq!(config.lifecycle.stop_timeout, Duration::from_secs(330));
    assert_eq!(config.lifecycle.startup_timeout, Duration::from_secs(60));
    assert_eq!(config.operations_bind_addr.to_string(), "127.0.0.1:9083");
    assert_eq!(config.databases.crawler.max_connections(), 16);
    assert_eq!(config.databases.business.max_connections(), 8);
    assert_eq!(config.spider_max_size_bytes, 8 * 1024 * 1024);
    assert_eq!(config.llm_rate_limit, CrawlerLlmRateLimitConfig::default());
    assert_eq!(config.cron.spider_interval, Duration::from_hours(72));
    assert_eq!(config.cron.scraper_interval, Duration::from_mins(10));
    assert!(!config.review_required);
    assert!(!config.url_pattern_review_required);
    assert!(config.review.bind_addr.ip().is_loopback());
    assert!(config.review.auth_token.is_none());
    assert!(config.cloudwatch.is_none());
    assert_eq!(
        config.product_schema_model,
        VertexAiConfig::new("fixture-project", "global", "gemini-3.1-pro-preview")
    );
    assert_eq!(
        config.url_classification_model,
        VertexAiConfig::new("fixture-project", "global", "gemini-3.1-flash-lite")
    );
    assert!(!format!("{config:?} {config:#?}").contains(CANARY));
    Ok(())
}

#[rstest]
#[case("LOCAL_DB_URL")]
#[case("BUSINESS_DATABASE_URL")]
#[case("STAGE")]
#[case("POSTGRES_SSL_MODE")]
#[case("SPIDER_MAX_SIZE_BYTES")]
#[case("VERTEX_AI_PROJECT_ID")]
#[case("VERTEX_AI_LOCATION")]
fn should_distinguish_missing_and_empty_required_inputs(#[case] key: &'static str) {
    let mut env = environment();
    env.remove(key);
    assert!(matches!(parse(&env), Err(ConfigError::Missing(actual)) if actual == key));
    for value in ["", " \t"] {
        env.insert(key, value.into());
        assert!(matches!(parse(&env), Err(ConfigError::Empty(actual)) if actual == key));
    }
}

#[rstest]
#[case("SPIDER_MAX_SIZE_BYTES", "1048575")]
#[case("SPIDER_MAX_SIZE_BYTES", "8388609")]
#[case("SPIDER_MAX_SIZE_BYTES", "+1048576")]
#[case("SPIDER_MAX_SIZE_BYTES", " 1048576")]
#[case("SPIDER_MAX_SIZE_BYTES", "18446744073709551616")]
#[case("CRAWLER_LLM_MAX_CONCURRENT_REQUESTS", "0")]
#[case("CRAWLER_LLM_MAX_CONCURRENT_REQUESTS", "18446744073709551616")]
#[case("CRAWLER_LLM_MAX_CONCURRENT_REQUESTS", "1.0")]
#[case("CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS", "0")]
#[case("CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS", "18446744073709551616")]
#[case("CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS", "-1")]
#[case("VERTEX_AI_PROJECT_ID", "private-secret-canary/path")]
#[case("VERTEX_AI_LOCATION", "https://private-secret-canary")]
#[case("VERTEX_AI_MODEL", "private-secret-canary?token")]
#[case("CRAWLER_VERTEX_AI_CHEAP_MODEL", "private-secret-canary/path")]
#[case(
    "CRAWLER_VERTEX_AI_URL_CLASSIFICATION_MODEL",
    "private-secret-canary/path"
)]
#[case("CRAWLER_SHUTDOWN_GRACE_SECONDS", "0")]
#[case("CRAWLER_SHUTDOWN_GRACE_SECONDS", "+300")]
#[case("CRAWLER_SHUTDOWN_GRACE_SECONDS", " 300")]
#[case("CRAWLER_SHUTDOWN_GRACE_SECONDS", "3601")]
#[case("CRAWLER_STOP_TIMEOUT_SECONDS", "329")]
#[case("CRAWLER_STOP_TIMEOUT_SECONDS", "3601")]
#[case("CRAWLER_STOP_TIMEOUT_SECONDS", "+330")]
#[case("CRAWLER_STARTUP_TIMEOUT_SECONDS", "0")]
#[case("CRAWLER_STARTUP_TIMEOUT_SECONDS", "3601")]
#[case("CRAWLER_STARTUP_TIMEOUT_SECONDS", "1.0")]
#[case("CRAWLER_STARTUP_TIMEOUT_SECONDS", "18446744073709551616")]
#[case("CRAWLER_OPERATIONS_BIND_ADDR", "private-secret-canary")]
#[case("CRAWLER_OPERATIONS_BIND_ADDR", "0.0.0.0:9083")]
#[case("CRAWLER_OPERATIONS_BIND_ADDR", "[::]:9083")]
#[case("CRAWLER_OPERATIONS_BIND_ADDR", "192.0.2.1:9083")]
#[case("CRAWLER_OPERATIONS_BIND_ADDR", "127.0.0.1:0")]
#[case("CRAWLER_OPERATIONS_BIND_ADDR", "[::1]:7878")]
#[case("CRAWLER_REVIEW_BIND_ADDR", "private-secret-canary")]
#[case("CRAWLER_REVIEW_BIND_ADDR", "127.0.0.1:0")]
#[case("CRAWLER_REVIEW_AUTH_TOKEN", "private-secret-canary\n")]
#[case("CRAWLER_REVIEW_REQUIRED", "tru")]
#[case("CRAWLER_REVIEW_URL_PATTERN_REQUIRED", "disabled")]
#[case("LOG_LEVEL", "private-secret-canary=invalid-level")]
#[case("CRAWLER_CLOUDWATCH_LOG_GROUP", "private-secret-canary:invalid")]
#[case("CRAWLER_CLOUDWATCH_LOG_STREAM", "private-secret-canary:invalid")]
#[case("GOOGLE_APPLICATION_CREDENTIALS", "private-secret-canary\n")]
fn should_reject_malformed_values_without_fallback_or_leak(
    #[case] key: &'static str,
    #[case] value: &str,
) {
    let mut env = environment();
    env.insert(key, value.into());
    let result = parse(&env);
    assert!(matches!(result, Err(ConfigError::Malformed { key: actual, .. }) if actual == key));
    if let Err(error) = result {
        assert_redacted(&error);
    }
}

#[test]
fn should_reject_empty_optional_settings_even_when_overridden() {
    for &key in INPUTS.iter().filter(|key| !key.starts_with("PG")) {
        let mut env = environment();
        env.insert(key, "".into());
        assert!(
            matches!(parse(&env), Err(ConfigError::Empty(actual)) if actual == key),
            "{key}"
        );
    }
}

#[test]
fn should_accept_numeric_boundaries_without_changing_rate_defaults() -> Result<(), ConfigError> {
    for (key, values) in [
        ("SPIDER_MAX_SIZE_BYTES", ["1048576", "8388608"]),
        ("CRAWLER_LLM_MAX_CONCURRENT_REQUESTS", ["1", "9"]),
        (
            "CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS",
            ["1", "18446744073709551615"],
        ),
    ] {
        for value in values {
            let mut env = environment();
            env.insert(key, value.into());
            parse(&env)?;
        }
    }
    Ok(())
}

#[test]
fn should_apply_validated_lifecycle_budget_before_database_validation() {
    let mut env = environment();
    env.insert("LOCAL_DB_URL", CANARY.into());
    env.insert("CRAWLER_STARTUP_TIMEOUT_SECONDS", "1".into());
    let configured = std::cell::Cell::new(false);
    let result = ServerConfig::from_lookup_with_lifecycle(
        Some(SHA),
        |key| env.get(key).cloned().ok_or(VarError::NotPresent),
        |config| {
            assert_eq!(config.startup_timeout, Duration::from_secs(1));
            configured.set(true);
        },
    );
    assert!(configured.get());
    assert!(matches!(result, Err(ConfigError::Database(_))));
    env.insert("CRAWLER_STARTUP_TIMEOUT_SECONDS", "+1".into());
    configured.set(false);
    assert!(
        ServerConfig::from_lookup_with_lifecycle(
            Some(SHA),
            |key| env.get(key).cloned().ok_or(VarError::NotPresent),
            |_| configured.set(true),
        )
        .is_err()
    );
    assert!(!configured.get());
}

#[test]
fn should_enforce_stage_drain_and_total_stop_budgets() -> Result<(), ConfigError> {
    for stage in ["local", "test", "ephemeral", "dev", "prod"] {
        let mut env = Environment(environment());
        env.0.insert("STAGE", stage.into());
        env.0.insert("CRAWLER_SHUTDOWN_GRACE_SECONDS", "1".into());
        env.0.insert("CRAWLER_STOP_TIMEOUT_SECONDS", "31".into());
        assert_eq!(
            LifecycleConfig::from_environment(&env).is_ok(),
            matches!(stage, "local" | "ephemeral" | "test")
        );
        env.0
            .insert("CRAWLER_SHUTDOWN_GRACE_SECONDS", "3570".into());
        env.0.insert("CRAWLER_STOP_TIMEOUT_SECONDS", "3600".into());
        for startup in ["1", "3600"] {
            env.0
                .insert("CRAWLER_STARTUP_TIMEOUT_SECONDS", startup.into());
            LifecycleConfig::from_environment(&env)?;
        }
        env.0
            .insert("CRAWLER_SHUTDOWN_GRACE_SECONDS", "3571".into());
        assert!(LifecycleConfig::from_environment(&env).is_err());
    }
    let mut env = environment();
    env.insert("CRAWLER_OPERATIONS_BIND_ADDR", "[::1]:9083".into());
    env.insert("CRAWLER_SHUTDOWN_GRACE_SECONDS", "1".into());
    env.insert("CRAWLER_STOP_TIMEOUT_SECONDS", "31".into());
    for stage in ["local", "ephemeral", "test"] {
        env.insert("STAGE", stage.into());
        parse(&env)?;
    }
    for stage in ["dev", "prod", "unknown"] {
        env.insert("STAGE", stage.into());
        assert!(parse(&env).is_err());
    }
    Ok(())
}

#[test]
fn should_preserve_positive_llm_values_up_to_semaphore_and_integer_limits()
-> Result<(), ConfigError> {
    let mut env = environment();
    for concurrency in [1, 9, tokio::sync::Semaphore::MAX_PERMITS] {
        env.insert(
            "CRAWLER_LLM_MAX_CONCURRENT_REQUESTS",
            concurrency.to_string(),
        );
        assert_eq!(
            parse(&env)?.llm_rate_limit.max_concurrent_requests,
            concurrency
        );
    }
    env.insert(
        "CRAWLER_LLM_MAX_CONCURRENT_REQUESTS",
        (tokio::sync::Semaphore::MAX_PERMITS + 1).to_string(),
    );
    assert!(matches!(
        parse(&env),
        Err(ConfigError::Malformed {
            key: "CRAWLER_LLM_MAX_CONCURRENT_REQUESTS",
            ..
        })
    ));
    env.insert("CRAWLER_LLM_MAX_CONCURRENT_REQUESTS", "+9".into());
    assert_eq!(parse(&env)?.llm_rate_limit.max_concurrent_requests, 9);
    env.remove("CRAWLER_LLM_MAX_CONCURRENT_REQUESTS");
    for milliseconds in [1, 1999, 2000, 60001, u64::MAX] {
        env.insert(
            "CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS",
            milliseconds.to_string(),
        );
        assert_eq!(
            parse(&env)?.llm_rate_limit.min_request_interval,
            Duration::from_millis(milliseconds)
        );
        env.insert(
            "CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS",
            format!("+{milliseconds}"),
        );
        assert_eq!(
            parse(&env)?.llm_rate_limit.min_request_interval,
            Duration::from_millis(milliseconds)
        );
    }
    Ok(())
}

#[test]
fn should_preserve_explicit_review_gates_and_require_auth_for_non_loopback()
-> Result<(), ConfigError> {
    let mut env = environment();
    env.insert("CRAWLER_REVIEW_BIND_ADDR", "0.0.0.0:7878".into());
    assert!(matches!(parse(&env), Err(ConfigError::Missing(_))));
    env.insert("CRAWLER_REVIEW_AUTH_TOKEN", CANARY.into());
    for value in ["true", "TRUE", "1", "yes", "YES"] {
        env.insert("CRAWLER_REVIEW_REQUIRED", value.into());
        env.insert("CRAWLER_REVIEW_URL_PATTERN_REQUIRED", value.into());
        let config = parse(&env)?;
        assert!(config.review_required && config.url_pattern_review_required);
        assert!(!format!("{config:?}").contains(CANARY));
    }
    for value in ["false", "FALSE", "0", "no", "NO"] {
        env.insert("CRAWLER_REVIEW_REQUIRED", value.into());
        assert!(!parse(&env)?.review_required);
    }
    Ok(())
}

#[test]
fn should_validate_google_and_cloudwatch_config_without_credentials() -> Result<(), ConfigError> {
    let mut env = environment();
    env.insert(
        "GOOGLE_APPLICATION_CREDENTIALS",
        "/not-opened/private-secret-canary.json".into(),
    );
    env.insert("CRAWLER_CLOUDWATCH_LOG_GROUP", "fixture-group".into());
    env.insert("HOSTNAME", "fixture-host".into());
    env.insert("CRAWLER_VERTEX_AI_CHEAP_MODEL", "fixture-model@001".into());
    let config = parse(&env)?;
    assert_eq!(
        config.cloudwatch,
        Some(CloudWatchLoggingConfig {
            log_group_name: "fixture-group".into(),
            log_stream_name: "fixture-host".into()
        })
    );
    assert_eq!(
        config.url_classification_model,
        VertexAiConfig::new("fixture-project", "global", "fixture-model@001")
    );
    Ok(())
}

#[test]
fn should_keep_shared_database_tls_url_and_ambient_policy() {
    for (key, value) in [
        ("STAGE", "prod"),
        ("STAGE", "dev"),
        ("STAGE", CANARY),
        ("POSTGRES_SSL_MODE", "require"),
        ("LOCAL_DB_URL", "postgres://private-secret-canary"),
        (
            "BUSINESS_DATABASE_URL",
            "postgres://u:private-secret-canary@127.0.0.1/business?sslmode=require",
        ),
        (
            "POSTGRES_SSL_ROOT_CERT",
            "/not-present/private-secret-canary.pem",
        ),
        ("PGOPTIONS", ""),
        ("PGSSLCERT", CANARY),
        ("PGSSLKEY", CANARY),
        ("PGSSLROOTCERT", CANARY),
    ] {
        let mut env = environment();
        env.insert(key, value.into());
        let result = parse(&env);
        assert!(matches!(result, Err(ConfigError::Database(_))), "{key}");
        if let Err(error) = result {
            assert_redacted(&error);
        }
    }
}

#[cfg(unix)]
#[test]
fn should_reject_non_unicode_arguments_and_every_owned_environment_input() {
    use std::os::unix::ffi::OsStringExt;
    let invalid = OsString::from_vec([CANARY.as_bytes(), &[0xff]].concat());
    for args in [
        vec![invalid.clone()],
        vec!["--help".into(), invalid.clone()],
        vec!["--check-config".into(), invalid.clone()],
    ] {
        assert_eq!(
            Mode::parse(args.into_iter()),
            Err(ArgumentError::NonUnicode)
        );
    }
    let env = environment();
    for &key in INPUTS {
        let result = ServerConfig::from_lookup(Some(SHA), |requested| {
            if requested == key {
                Err(VarError::NotUnicode(invalid.clone()))
            } else {
                env.get(requested).cloned().ok_or(VarError::NotPresent)
            }
        });
        assert!(matches!(result, Err(ConfigError::NonUnicode(actual)) if actual == key));
        if let Err(error) = result {
            assert_redacted(&error);
        }
    }
}

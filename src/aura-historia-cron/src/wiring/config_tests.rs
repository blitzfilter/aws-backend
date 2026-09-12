use super::*;
use std::{collections::BTreeMap, ffi::OsString};

type TestResult = Result<(), Box<dyn Error>>;
const SCHEDULE: &str = "SEARCH_FILTER_PERIODIC_MATCH_CRON";
const NUMBERS: &[&str] = &[
    "PERIODIC_MATCH_FILTER_PAGE_SIZE",
    "PERIODIC_MATCH_HYBRID_SCAN_LIMIT",
    "PERIODIC_MATCH_EVALUATION_LIMIT",
    "PERIODIC_MATCH_LLM_CONCURRENCY",
    "PERIODIC_MATCH_MAX_ATTEMPTS",
    "PERIODIC_MATCH_MAX_RUN_SECONDS",
    "PERIODIC_MATCH_PROJECTION_LAG_SECONDS",
    "PERIODIC_MATCH_REPLAY_OVERLAP_SECONDS",
];

fn inputs() -> BTreeMap<&'static str, OsString> {
    [
        ("STAGE", "test"),
        ("POSTGRES_SSL_MODE", "disable"),
        ("POSTGRES_HOST", "postgres.example.test"),
        ("POSTGRES_DATABASE", "database_canary"),
        ("POSTGRES_USERNAME", "username_canary"),
        ("POSTGRES_PASSWORD", "password_canary"),
        ("OPENSEARCH_ENDPOINT_URL", "https://opensearch.example.test"),
        ("VERTEX_AI_PROJECT_ID", "project_canary"),
        ("VERTEX_AI_LOCATION", "location_canary"),
        ("VERTEX_AI_MODEL", "model_canary"),
    ]
    .map(|(name, value)| (name, value.into()))
    .into()
}

fn parse(values: &BTreeMap<&'static str, OsString>) -> Result<PeriodicMatchConfig, WiringError> {
    PeriodicMatchConfig::from_lookup(&mut |name| {
        values
            .get(name)
            .cloned()
            .ok_or(VarError::NotPresent)?
            .into_string()
            .map_err(VarError::NotUnicode)
    })
}

#[test]
fn should_use_job_defaults_only_when_inputs_are_absent() -> TestResult {
    let config = parse(&inputs())?;
    assert_eq!(config.schedule, "0 0 15 * * * *");
    assert_eq!(config.max_run_duration, Duration::from_secs(7200));
    assert_eq!(config.policy.filter_page_size.get(), 100);
    assert_eq!(config.policy.hybrid_scan_limit.get(), 100);
    assert_eq!(config.policy.evaluation_limit.get(), 50);
    assert_eq!(config.policy.llm_concurrency.get(), 8);
    assert_eq!(config.policy.max_attempts.get(), 3);
    assert_eq!(config.policy.projection_lag.whole_seconds(), 900);
    assert_eq!(config.policy.replay_overlap.whole_seconds(), 7200);
    Ok(())
}

#[test]
fn should_keep_job_input_trimming_and_zero_lag_support() -> TestResult {
    let mut values = inputs();
    values.insert(SCHEDULE, "  0 1 15 * * * *  ".into());
    for &name in NUMBERS {
        values.insert(name, " 1 ".into());
    }
    values.insert("PERIODIC_MATCH_PROJECTION_LAG_SECONDS", " 0 ".into());
    values.insert("PERIODIC_MATCH_REPLAY_OVERLAP_SECONDS", " 0 ".into());
    let config = parse(&values)?;
    assert_eq!(config.schedule, "0 1 15 * * * *");
    assert_eq!(config.max_run_duration, Duration::from_secs(1));
    assert_eq!(config.policy.filter_page_size.get(), 1);
    assert_eq!(config.policy.hybrid_scan_limit.get(), 1);
    assert_eq!(config.policy.evaluation_limit.get(), 1);
    assert_eq!(config.policy.llm_concurrency.get(), 1);
    assert_eq!(config.policy.max_attempts.get(), 1);
    assert_eq!(config.policy.projection_lag.whole_seconds(), 0);
    assert_eq!(config.policy.replay_overlap.whole_seconds(), 0);
    Ok(())
}

#[test]
fn should_reject_present_empty_and_malformed_job_inputs_during_config_parsing() -> TestResult {
    for name in std::iter::once(SCHEDULE).chain(NUMBERS.iter().copied()) {
        for value in ["", " \t\n ", "value_canary"] {
            let mut values = inputs();
            values.insert(name, value.into());
            let error = parse(&values).err().ok_or("invalid job input accepted")?;
            if name == SCHEDULE {
                assert!(matches!(error, WiringError::InvalidSchedule { .. }));
            } else {
                assert!(
                    matches!(error, WiringError::InvalidNumber { name: key, .. } if key == name)
                );
            }
            super::error_tests::assert_redacted_chain(&error);
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn should_reject_non_unicode_job_inputs_and_retain_the_original_var_error() -> TestResult {
    use std::os::unix::ffi::OsStringExt;

    for name in std::iter::once(SCHEDULE).chain(NUMBERS.iter().copied()) {
        let mut values = inputs();
        let value = OsString::from_vec(b"value_canary\xff".to_vec());
        values.insert(name, value.clone());
        let error = parse(&values)
            .err()
            .ok_or("non-Unicode job input accepted")?;
        assert!(matches!(error, WiringError::InvalidEnvEncoding { name: key, .. } if key == name));
        super::error_tests::assert_redacted_chain(&error);
        let source = super::error_tests::original::<VarError>(&error)?;
        assert_eq!(source, &VarError::NotUnicode(value));
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn should_not_treat_non_unicode_required_inputs_as_missing() -> TestResult {
    use std::os::unix::ffi::OsStringExt;

    for name in [
        "STAGE",
        "OPENSEARCH_ENDPOINT_URL",
        "OPENSEARCH_USERNAME",
        "OPENSEARCH_PASSWORD",
        "VERTEX_AI_PROJECT_ID",
        "VERTEX_AI_LOCATION",
        "VERTEX_AI_MODEL",
    ] {
        let mut values = inputs();
        values.insert("STAGE", "prod".into());
        values.insert("POSTGRES_SSL_MODE", "verify-full".into());
        values.insert(
            "POSTGRES_SSL_ROOT_CERT",
            concat!(env!("CARGO_MANIFEST_DIR"), "/src/postgres-test-ca.crt").into(),
        );
        values.insert("OPENSEARCH_USERNAME", "username_canary".into());
        values.insert("OPENSEARCH_PASSWORD", "password_canary".into());
        values.insert(name, OsString::from_vec(b"value_canary\xff".to_vec()));
        let error = parse(&values)
            .err()
            .ok_or("non-Unicode required input accepted")?;
        assert!(matches!(error, WiringError::InvalidEnvEncoding { name: key, .. } if key == name));
        super::error_tests::assert_redacted_chain(&error);
        assert!(matches!(
            super::error_tests::original::<VarError>(&error)?,
            VarError::NotUnicode(_)
        ));
    }
    Ok(())
}

#[test]
fn should_validate_full_job_config_without_building_adapters() -> TestResult {
    for name in [
        "VERTEX_AI_PROJECT_ID",
        "VERTEX_AI_LOCATION",
        "VERTEX_AI_MODEL",
    ] {
        let mut values = inputs();
        values.remove(name);
        assert!(
            matches!(parse(&values), Err(WiringError::MissingEnv { name: key }) if key == name)
        );
    }
    let mut values = inputs();
    values.insert("PERIODIC_MATCH_MAX_RUN_SECONDS", "7201".into());
    assert!(matches!(parse(&values), Err(WiringError::InvalidPolicy)));
    values.insert("PERIODIC_MATCH_MAX_RUN_SECONDS", "0".into());
    assert!(matches!(parse(&values), Err(WiringError::InvalidPolicy)));
    values.insert("PERIODIC_MATCH_MAX_RUN_SECONDS", "7200".into());
    assert!(parse(&values).is_ok());
    for name in [
        "PERIODIC_MATCH_PROJECTION_LAG_SECONDS",
        "PERIODIC_MATCH_REPLAY_OVERLAP_SECONDS",
    ] {
        let mut values = inputs();
        values.insert(name, u64::MAX.to_string().into());
        let error = parse(&values).err().ok_or("duration overflow accepted")?;
        assert!(matches!(error, WiringError::InvalidNumber { name: key, .. } if key == name));
        super::error_tests::assert_redacted_chain(&error);
        super::error_tests::original::<std::num::TryFromIntError>(&error)?;
    }
    Ok(())
}

//! TTL fault setup only. Parent owns a distinct fresh business DB/role per case,
//! invokes the real initializer, and verifies cleanup through the existing supervisor.
use super::{Database, SQL, Target, TestError, TestResult, invoke, snapshot, unchanged};
use serde_json::Value;
use std::path::Path;
use tokio::time::timeout;

#[derive(Clone, Copy, Debug)]
pub(crate) enum TtlFault {
    ExtraIndex,
    ExtraConstraint,
    ExtraDefault,
    ChangedDefault,
    MissingDefault,
    ExtraColumn,
    ChangedType,
    ChangedNullability,
    ChangedPrimaryKey,
    ConfigRow,
    RowLevelSecurity,
    StorageOptions,
    MissingMembership,
    WrongVersion,
}

impl TtlFault {
    pub(super) const ALL: [Self; 14] = [
        Self::ExtraIndex,
        Self::ExtraConstraint,
        Self::ExtraDefault,
        Self::ChangedDefault,
        Self::MissingDefault,
        Self::ExtraColumn,
        Self::ChangedType,
        Self::ChangedNullability,
        Self::ChangedPrimaryKey,
        Self::ConfigRow,
        Self::RowLevelSecurity,
        Self::StorageOptions,
        Self::MissingMembership,
        Self::WrongVersion,
    ];

    fn sql(self) -> &'static str {
        match self {
            Self::ExtraIndex => "CREATE INDEX fixture_extra ON public.ttl_index_table (table_name)",
            Self::ExtraConstraint => {
                "ALTER TABLE public.ttl_index_table ADD CONSTRAINT fixture_extra CHECK (batch_size > 0)"
            }
            Self::ExtraDefault => {
                "ALTER TABLE public.ttl_index_table ALTER COLUMN last_run SET DEFAULT now()"
            }
            Self::ChangedDefault => {
                "ALTER TABLE public.ttl_index_table ALTER COLUMN batch_size SET DEFAULT 99"
            }
            Self::MissingDefault => {
                "ALTER TABLE public.ttl_index_table ALTER COLUMN active DROP DEFAULT"
            }
            Self::ExtraColumn => "ALTER TABLE public.ttl_index_table ADD COLUMN fixture_extra text",
            Self::ChangedType => {
                "ALTER TABLE public.ttl_index_table ALTER COLUMN batch_size TYPE bigint"
            }
            Self::ChangedNullability => {
                "ALTER TABLE public.ttl_index_table ALTER COLUMN active DROP NOT NULL"
            }
            Self::ChangedPrimaryKey => {
                "ALTER TABLE public.ttl_index_table DROP CONSTRAINT ttl_index_table_pkey; ALTER TABLE public.ttl_index_table ADD PRIMARY KEY (schema_name, table_name)"
            }
            Self::ConfigRow => {
                "INSERT INTO public.ttl_index_table (table_name,column_name,expire_after_seconds) VALUES ('fixture_absent','fixture_absent',60)"
            }
            Self::RowLevelSecurity => {
                "ALTER TABLE public.ttl_index_table ENABLE ROW LEVEL SECURITY"
            }
            Self::StorageOptions => "ALTER TABLE public.ttl_index_table SET (fillfactor = 90)",
            Self::MissingMembership => {
                "ALTER EXTENSION pg_ttl_index DROP TABLE public.ttl_index_table"
            }
            Self::WrongVersion => {
                "UPDATE pg_catalog.pg_extension SET extversion = '0.0.0' WHERE extname = 'pg_ttl_index'"
            }
        }
    }

    fn snapshot_pointer(self) -> &'static str {
        match self {
            Self::ExtraIndex => "/catalogs/pg_index",
            Self::ExtraConstraint | Self::ChangedPrimaryKey => "/catalogs/pg_constraint",
            Self::ExtraDefault | Self::ChangedDefault | Self::MissingDefault => {
                "/catalogs/pg_attrdef"
            }
            Self::ExtraColumn | Self::ChangedType | Self::ChangedNullability => {
                "/catalogs/pg_attribute"
            }
            Self::ConfigRow => "/state/public.ttl_index_table",
            Self::RowLevelSecurity | Self::StorageOptions => "/catalogs/pg_class",
            Self::MissingMembership => "/catalogs/pg_depend",
            Self::WrongVersion => "/catalogs/pg_extension",
        }
    }

    fn assert_fault_visible(self, pristine: &Value, injected: &Value) -> TestResult {
        match (
            pristine.pointer(self.snapshot_pointer()),
            injected.pointer(self.snapshot_pointer()),
        ) {
            (Some(before), Some(after)) if before != after => Ok(()),
            _ => Err(TestError::failure("CASE_ASSERTION")),
        }
    }

    pub(super) async fn check(self, database: &Database, directory: &Path) -> TestResult {
        let pristine = snapshot(database).await?;
        // Committed fault, not an uncommitted transaction invisible to the child CLI.
        timeout(SQL, sqlx::raw_sql(self.sql()).execute(&database.pool)).await??;
        let before = snapshot(database).await?;
        self.assert_fault_visible(&pristine, &before)?;
        let result = invoke(
            database,
            directory,
            Target::Business,
            "--initialize-fresh",
            4,
            "NOT_FRESH",
        )
        .await;
        // Preserve the existing refusal contract even when CLI outcome assertion fails.
        unchanged(database, &before).await?;
        result
    }
}

#[test]
fn should_preserve_fourteen_distinct_ttl_faults() {
    let sql: std::collections::BTreeSet<_> = TtlFault::ALL.into_iter().map(TtlFault::sql).collect();
    assert_eq!(sql.len(), 14);
}

#[test]
fn should_require_each_faults_catalog_or_row_evidence_without_printing_snapshots() -> TestResult {
    let pristine = serde_json::json!({
        "state": {"public.ttl_index_table": []},
        "catalogs": {"pg_class": [], "pg_attribute": [], "pg_attrdef": [], "pg_depend": [],
            "pg_extension": [], "pg_constraint": [], "pg_index": []},
    });
    for fault in TtlFault::ALL {
        assert!(fault.assert_fault_visible(&pristine, &pristine).is_err());
        assert!(fault.assert_fault_visible(&Value::Null, &pristine).is_err());
        let mut injected = pristine.clone();
        let section = injected
            .pointer_mut(fault.snapshot_pointer())
            .ok_or_else(|| TestError::failure("CASE_ASSERTION"))?;
        *section = serde_json::json!(["changed-fixture-metadata"]);
        fault.assert_fault_visible(&pristine, &injected)?;
        let mut unrelated = pristine.clone();
        unrelated["unrelated"] = serde_json::json!(["private-provider-body"]);
        assert!(fault.assert_fault_visible(&pristine, &unrelated).is_err());
    }
    Ok(())
}

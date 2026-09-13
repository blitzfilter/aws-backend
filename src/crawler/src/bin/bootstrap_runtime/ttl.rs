//! Narrow pg_ttl_index 3.0.0 prerequisite exception. Vendor source pin lives in
//! test-api/postgres/Dockerfile (zip SHA256 cd27090bf5496b7563ed0926cebce5424864db7f6c149e1da4b75042439e2d82).
//! Validate structure before permitting specific automatic dependents; never traverse
//! arbitrary `a` edges. Trusted administrators, not function-body tamper attestation.
use super::{Code, Failure};
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct Table {
    oid: i64,
    toast: i64,
    class_catalog: i64,
    default_catalog: i64,
    constraint_catalog: i64,
}

#[derive(Clone, sqlx::FromRow)]
struct Column {
    number: i16,
    name: String,
    type_name: String,
    not_null: bool,
    ordinary: bool,
    default_oid: Option<i64>,
    default_expression: Option<String>,
}

// Order, types, nullability and deparsed defaults from the pinned vendor CREATE TABLE.
const COLUMNS: &[(&str, &str, bool, Option<&str>)] = &[
    ("schema_name", "text", true, Some("'public'::text")),
    ("table_name", "text", true, None),
    ("column_name", "text", true, None),
    ("expire_after_seconds", "integer", true, None),
    ("active", "boolean", true, Some("true")),
    (
        "created_at",
        "timestamp with time zone",
        true,
        Some("now()"),
    ),
    (
        "updated_at",
        "timestamp with time zone",
        false,
        Some("now()"),
    ),
    ("last_run", "timestamp with time zone", false, None),
    ("batch_size", "integer", true, Some("10000")),
    ("rows_deleted_last_run", "bigint", false, Some("0")),
    ("total_rows_deleted", "bigint", false, Some("0")),
    ("index_name", "text", false, None),
    ("soft_delete_column", "text", false, None),
    ("index_created_by_extension", "boolean", true, Some("false")),
];

fn columns_match(columns: &[Column]) -> bool {
    columns.len() == COLUMNS.len()
        && columns.iter().zip(COLUMNS).enumerate().all(
            |(position, (column, (name, type_name, not_null, default)))| {
                usize::try_from(column.number) == Ok(position + 1)
                    && column.name == *name
                    && column.type_name == *type_name
                    && column.not_null == *not_null
                    && column.ordinary
                    && column.default_expression.as_deref() == *default
                    && column.default_oid.is_some() == default.is_some()
            },
        )
}

#[derive(sqlx::FromRow)]
struct PrimaryKey {
    oid: i64,
    index: i64,
    exact: bool,
}

#[derive(Clone, sqlx::FromRow)]
struct Index {
    oid: i64,
    parent: i64,
    primary: bool,
    ordinary: bool,
    definition: String,
    linked: bool,
}

fn indexes_match(table: &Table, primary: &PrimaryKey, indexes: &[Index]) -> bool {
    if indexes.len() != 2 {
        return false;
    }
    let toast_definition = format!(
        "CREATE UNIQUE INDEX pg_toast_{}_index ON pg_toast.pg_toast_{} USING btree (chunk_id, chunk_seq)",
        table.oid, table.oid,
    );
    let primary_definition = "CREATE UNIQUE INDEX ttl_index_table_pkey ON public.ttl_index_table USING btree (schema_name, table_name, column_name)";
    indexes
        .iter()
        .filter(|index| index.parent == table.oid)
        .count()
        == 1
        && indexes
            .iter()
            .filter(|index| index.parent == table.toast)
            .count()
            == 1
        && indexes.iter().all(|index| {
            index.ordinary
                && index.linked
                && if index.parent == table.oid {
                    index.primary
                        && index.oid == primary.index
                        && index.definition == primary_definition
                } else {
                    // PostgreSQL 16 marks its internal toast index primary too.
                    index.primary && index.definition == toast_definition
                }
        })
}

pub(super) async fn allowance(connection: &mut PgConnection) -> Result<Vec<(i64, i64)>, Failure> {
    let table: Table = sqlx::query_as(
        "SELECT c.oid::bigint AS oid, c.reltoastrelid::bigint AS toast,
             'pg_catalog.pg_class'::regclass::bigint AS class_catalog,
             'pg_catalog.pg_attrdef'::regclass::bigint AS default_catalog,
             'pg_catalog.pg_constraint'::regclass::bigint AS constraint_catalog
         FROM pg_catalog.pg_class c
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
         JOIN pg_catalog.pg_extension e ON e.extname = 'pg_ttl_index'
         JOIN pg_catalog.pg_namespace en ON en.oid = e.extnamespace
         JOIN pg_catalog.pg_class t ON t.oid = c.reltoastrelid
         JOIN pg_catalog.pg_namespace tn ON tn.oid = t.relnamespace
         WHERE e.extversion = '3.0.0' AND en.nspname = 'public'
           AND n.nspname = 'public' AND c.relname = 'ttl_index_table'
           AND c.relkind = 'r' AND c.relpersistence = 'p'
           AND NOT c.relrowsecurity AND NOT c.relforcerowsecurity AND NOT c.relispartition
           AND NOT c.relhassubclass AND c.reloptions IS NULL
           AND t.relkind = 't' AND t.relpersistence = 'p' AND tn.nspname = 'pg_toast'
           AND t.relname = 'pg_toast_' || c.oid::text
           AND EXISTS (SELECT FROM pg_catalog.pg_depend d
             WHERE d.classid = 'pg_catalog.pg_class'::regclass AND d.objid = c.oid AND d.objsubid = 0
               AND d.refclassid = 'pg_catalog.pg_extension'::regclass AND d.refobjid = e.oid
               AND d.refobjsubid = 0 AND d.deptype = 'e')
           AND EXISTS (SELECT FROM pg_catalog.pg_depend d
             WHERE d.classid = 'pg_catalog.pg_class'::regclass AND d.objid = t.oid AND d.objsubid = 0
               AND d.refclassid = 'pg_catalog.pg_class'::regclass AND d.refobjid = c.oid
               AND d.refobjsubid = 0 AND d.deptype = 'i')"
    ).fetch_optional(&mut *connection).await?
        .ok_or_else(|| Failure::new(Code::NotFresh))?;

    let columns: Vec<Column> = sqlx::query_as(
        "SELECT a.attnum AS number, a.attname AS name,
             pg_catalog.format_type(a.atttypid, a.atttypmod) AS type_name, a.attnotnull AS not_null,
             NOT a.attisdropped AND a.attislocal AND a.attinhcount = 0
               AND a.attidentity = '' AND a.attgenerated = ''
               AND a.attcollation = CASE WHEN a.atttypid = 'pg_catalog.text'::regtype
                 THEN 'pg_catalog.\"default\"'::regcollation::oid ELSE 0::oid END
               AND (d.oid IS NULL OR EXISTS (SELECT FROM pg_catalog.pg_depend dep
                 WHERE dep.classid = 'pg_catalog.pg_attrdef'::regclass AND dep.objid = d.oid
                   AND dep.objsubid = 0 AND dep.refclassid = 'pg_catalog.pg_class'::regclass
                   AND dep.refobjid = a.attrelid AND dep.refobjsubid = a.attnum AND dep.deptype = 'a')) AS ordinary,
             d.oid::bigint AS default_oid, pg_catalog.pg_get_expr(d.adbin, d.adrelid, false) AS default_expression
         FROM pg_catalog.pg_attribute a LEFT JOIN pg_catalog.pg_attrdef d
           ON d.adrelid = a.attrelid AND d.adnum = a.attnum
         WHERE a.attrelid::bigint = $1 AND a.attnum > 0 ORDER BY a.attnum LIMIT 15"
    ).bind(table.oid).fetch_all(&mut *connection).await?;
    if !columns_match(&columns) {
        return Err(Failure::new(Code::NotFresh));
    }
    let constraints: Vec<PrimaryKey> = sqlx::query_as(
        "SELECT c.oid::bigint AS oid, c.conindid::bigint AS index,
             c.contype = 'p' AND c.conname = 'ttl_index_table_pkey'
               AND c.conkey = ARRAY[1,2,3]::smallint[] AND c.confrelid = 0
               AND NOT c.condeferrable AND NOT c.condeferred AND c.convalidated
               AND c.conislocal AND c.coninhcount = 0 AND c.conparentid = 0 AND c.conbin IS NULL
               AND pg_catalog.pg_get_constraintdef(c.oid, false) = 'PRIMARY KEY (schema_name, table_name, column_name)'
               AND (SELECT count(*) FROM pg_catalog.pg_depend d
                 WHERE d.classid = 'pg_catalog.pg_constraint'::regclass AND d.objid = c.oid
                   AND d.objsubid = 0 AND d.refclassid = 'pg_catalog.pg_class'::regclass
                   AND d.refobjid = c.conrelid AND d.refobjsubid IN (1,2,3) AND d.deptype = 'a') = 3 AS exact
         FROM pg_catalog.pg_constraint c WHERE c.conrelid::bigint = $1 LIMIT 2"
    ).bind(table.oid).fetch_all(&mut *connection).await?;
    let [primary] = constraints.as_slice() else {
        return Err(Failure::new(Code::NotFresh));
    };
    if !primary.exact {
        return Err(Failure::new(Code::NotFresh));
    }

    let indexes: Vec<Index> = sqlx::query_as(
        "SELECT c.oid::bigint AS oid, i.indrelid::bigint AS parent, i.indisprimary AS primary,
             c.relkind = 'i' AND c.relpersistence = 'p' AND c.reloptions IS NULL
               AND am.amname = 'btree' AND i.indisunique AND i.indimmediate
               AND i.indisvalid AND i.indisready AND i.indislive AND NOT i.indisexclusion
               AND NOT i.indnullsnotdistinct AND i.indexprs IS NULL AND i.indpred IS NULL
               AND i.indnatts = i.indnkeyatts
               AND ((i.indrelid::bigint = $1 AND i.indkey::text = '1 2 3' AND i.indoption::text = '0 0 0')
                 OR (i.indrelid::bigint = $2 AND i.indkey::text = '1 2' AND i.indoption::text = '0 0')) AS ordinary,
             pg_catalog.pg_get_indexdef(c.oid, 0, false) AS definition,
             CASE WHEN i.indrelid::bigint = $1 THEN EXISTS (
               SELECT FROM pg_catalog.pg_depend d WHERE d.classid = 'pg_catalog.pg_class'::regclass
                 AND d.objid = c.oid AND d.objsubid = 0 AND d.refclassid = 'pg_catalog.pg_constraint'::regclass
                 AND d.refobjid::bigint = $3 AND d.refobjsubid = 0 AND d.deptype = 'i')
             ELSE (SELECT count(*) FROM pg_catalog.pg_depend d WHERE d.classid = 'pg_catalog.pg_class'::regclass
                 AND d.objid = c.oid AND d.objsubid = 0 AND d.refclassid = 'pg_catalog.pg_class'::regclass
                 AND d.refobjid = i.indrelid AND d.refobjsubid IN (1,2) AND d.deptype = 'a') = 2 END AS linked
         FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid = i.indexrelid
         JOIN pg_catalog.pg_am am ON am.oid = c.relam
         WHERE i.indrelid::bigint IN ($1, $2) ORDER BY c.oid LIMIT 3"
    ).bind(table.oid).bind(table.toast).bind(primary.oid).fetch_all(&mut *connection).await?;
    if !indexes_match(&table, primary, &indexes) {
        return Err(Failure::new(Code::NotFresh));
    }
    // RLS/partition/view substitutions already refused. Read presence only, never config values.
    let populated: bool = sqlx::query_scalar("SELECT EXISTS (SELECT FROM public.ttl_index_table)")
        .fetch_one(&mut *connection)
        .await?;
    if populated {
        return Err(Failure::new(Code::NotFresh));
    }
    let mut allowed: Vec<_> = columns
        .iter()
        .filter_map(|column| column.default_oid)
        .map(|oid| (table.default_catalog, oid))
        .collect();
    allowed.push((table.constraint_catalog, primary.oid));
    allowed.extend(indexes.iter().map(|index| (table.class_catalog, index.oid)));
    Ok(allowed)
}

#[cfg(test)]
#[path = "ttl_tests.rs"]
mod tests;

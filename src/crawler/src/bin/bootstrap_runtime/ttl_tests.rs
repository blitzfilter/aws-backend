use super::*;

fn columns() -> Vec<Column> {
    COLUMNS
        .iter()
        .enumerate()
        .map(|(index, (name, type_name, not_null, default))| Column {
            number: (index + 1) as i16,
            name: (*name).into(),
            type_name: (*type_name).into(),
            not_null: *not_null,
            ordinary: true,
            default_oid: default.map(|_| 20000 + index as i64),
            default_expression: default.map(str::to_owned),
        })
        .collect()
}

#[test]
fn should_require_all_fourteen_exact_vendor_columns_and_eight_defaults() {
    let original = columns();
    assert!(columns_match(&original));
    assert_eq!(
        original
            .iter()
            .filter(|column| column.default_oid.is_some())
            .count(),
        8
    );
    for position in 0..original.len() {
        for change in 0..7 {
            let mut changed = original.clone();
            let column = &mut changed[position];
            match change {
                0 => column.number = 0,
                1 => column.name.push_str("_extra"),
                2 => column.type_name = "uuid".into(),
                3 => column.not_null = !column.not_null,
                4 => column.ordinary = false,
                5 => column.default_expression = Some("unexpected_function()".into()),
                _ => {
                    column.default_oid = if column.default_oid.is_some() {
                        None
                    } else {
                        Some(1)
                    }
                }
            }
            assert!(!columns_match(&changed));
        }
    }
    assert!(!columns_match(&original[..13]));
    let mut extra = original.clone();
    extra.push(original[0].clone());
    assert!(!columns_match(&extra));
}

#[test]
fn should_allow_only_exact_primary_and_internal_toast_indexes() {
    let table = Table {
        oid: 20000,
        toast: 20001,
        class_catalog: 1,
        default_catalog: 2,
        constraint_catalog: 3,
    };
    let primary = PrimaryKey {
        oid: 20002,
        index: 20003,
        exact: true,
    };
    let indexes = vec![
        Index { oid: 20003, parent: 20000, primary: true, ordinary: true, linked: true,
            definition: "CREATE UNIQUE INDEX ttl_index_table_pkey ON public.ttl_index_table USING btree (schema_name, table_name, column_name)".into() },
        Index { oid: 20004, parent: 20001, primary: true, ordinary: true, linked: true,
            definition: "CREATE UNIQUE INDEX pg_toast_20000_index ON pg_toast.pg_toast_20000 USING btree (chunk_id, chunk_seq)".into() },
    ];
    assert!(indexes_match(&table, &primary, &indexes));
    assert!(!indexes_match(&table, &primary, &indexes[..1]));
    let mut extra = indexes.clone();
    extra.push(indexes[0].clone());
    assert!(!indexes_match(&table, &primary, &extra));
    for position in 0..2 {
        for change in 0..5 {
            let mut changed = indexes.clone();
            match change {
                0 => changed[position].parent = 30000,
                1 => changed[position].primary = !changed[position].primary,
                2 => changed[position].ordinary = false,
                3 => changed[position].linked = false,
                _ => changed[position].definition.push_str(" WHERE true"),
            }
            assert!(!indexes_match(&table, &primary, &changed));
        }
    }
}

// Real PG acceptance and all 14 TTL negatives now live in the supervised
// server_preflight_postgres fresh_bootstrap CLI fixture. Its parent checks exact
// container removal after process exit; this file retains private policy unit tests.

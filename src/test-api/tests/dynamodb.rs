use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_run_without_errors() {}

#[localstack_test(services = [DynamoDB()])]
async fn should_set_up_tables() {
    let list_tables_output = get_dynamodb_client()
        .await
        .list_tables()
        .send()
        .await
        .unwrap();
    let tables = list_tables_output.table_names();

    assert_eq!(tables.len(), 1);
    assert!(tables.contains(&"table_1".to_string()));
}

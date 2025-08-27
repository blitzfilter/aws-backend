use staging_tests::{get_cfn_output, staging_test};

#[staging_test]
async fn should_respond_404_when_item_does_not_exist() {
    let response = reqwest::get(format!(
        "{}/items/foo/bar",
        get_cfn_output().api_gateway_endpoint_url
    ))
    .await
    .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("ITEM_NOT_FOUND", body["error"]);
}

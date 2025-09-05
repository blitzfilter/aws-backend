use aws_tests_common::get_cfn_output;
use smoking_tests::smoking_test;
use uuid::Uuid;

#[smoking_test]
async fn should_respond_404_when_item_does_not_exist_in_dynamodb() {
    let response = reqwest::get(format!(
        "{}/api/v1/items/{}/{}",
        get_cfn_output().api_gateway_endpoint_url,
        Uuid::new_v4(),
        Uuid::new_v4()
    ))
    .await
    .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("ITEM_NOT_FOUND", body["error"]);
}

#[smoking_test]
async fn should_respond_200_when_hits_for_opensearch() {
    let response = reqwest::get(format!(
        "{}/api/v1/items?q=Chopin%20Etudes&language=en&currency=EUR&sort=price&order=asc&from=0&size=5",
        get_cfn_output().api_gateway_endpoint_url
    ))
    .await
    .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert!(body["items"].is_array());
    assert!(body["pagination"]["from"].is_u64());
    assert!(body["pagination"]["size"].is_u64());
    assert!(body["pagination"]["total"].is_u64());
}

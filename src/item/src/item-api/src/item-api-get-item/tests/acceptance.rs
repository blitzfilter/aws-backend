use std::collections::HashMap;

use aws_lambda_events::apigw::ApiGatewayProxyRequest;
use common::{shop_id::ShopId, shops_item_id::ShopsItemId};
use item_api_get_item::handler;
use lambda_runtime::LambdaEvent;
use test_api::*;

#[rstest::rstest]
#[test_attr(apply(test))]
#[case("abcdefg", "123456")]
#[case("boop", "bap")]
#[case("foo", "bar")]
#[case(&ShopId::new().to_string(), &ShopsItemId::new().to_string())]
#[localstack_test(services = [DynamoDB()])]
async fn should_return_404_when_item_does_not_exist(
    #[case] shop_id: &str,
    #[case] shops_item_id: &str,
) {
    let lambda_event = LambdaEvent {
        payload: ApiGatewayProxyRequest {
            resource: None,
            path: None,
            http_method: Default::default(),
            headers: Default::default(),
            multi_value_headers: Default::default(),
            query_string_parameters: Default::default(),
            multi_value_query_string_parameters: Default::default(),
            path_parameters: HashMap::from_iter([
                ("shopId".to_string(), shop_id.to_string()),
                ("shopsItemId".to_string(), shops_item_id.to_string()),
            ]),
            stage_variables: Default::default(),
            request_context: Default::default(),
            body: None,
            is_base64_encoded: false,
        },
        context: Default::default(),
    };
    let service = get_dynamodb_client().await;

    let actual = handler(lambda_event, service).await.unwrap();

    assert_eq!(404, actual.status_code);
}

use test_api::localstack::spin_up_localstack_with_services;

#[tokio::test]
#[serial_test::serial]
async fn should_expose_test_host_and_port() {
    let container = spin_up_localstack_with_services(&[]).await;

    let host_ip = container.get_host().await.ok();
    let host_port = container.get_host_port_ipv4(4566).await.ok();

    assert_eq!(host_ip.unwrap().to_string(), "localhost");
    assert_eq!(host_port.unwrap(), 4566);

    drop(container);
}

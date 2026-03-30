use bcc_client::{error::ClientError, rpc::RpcClient};
use mockito::Server;

#[tokio::test]
async fn get_tip_parses_success_response() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/chain/tip")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"height":42,"hash":"abcd1234"}"#)
        .create_async()
        .await;

    let client = RpcClient::new(server.url());
    let tip    = client.get_tip().await.unwrap();

    assert_eq!(tip.height, 42);
    assert_eq!(tip.hash, "abcd1234");
    mock.assert_async().await;
}

#[tokio::test]
async fn get_balance_parses_success_response() {
    let mut server = Server::new_async().await;
    let addr = "bcs1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let mock = server
        .mock("GET", format!("/balance/{addr}").as_str())
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(r#"{{"address":"{addr}","balance":1000}}"#))
        .create_async()
        .await;

    let client = RpcClient::new(server.url());
    let resp   = client.get_balance(addr).await.unwrap();

    assert_eq!(resp.balance, 1000);
    mock.assert_async().await;
}

#[tokio::test]
async fn get_utxos_parses_success_response() {
    let mut server = Server::new_async().await;
    let addr  = "bcs1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let hash  = hex::encode([0u8; 32]);
    let body  = format!(r#"[{{"tx_hash":"{hash}","index":0,"amount":500}}]"#);
    let mock  = server
        .mock("GET", format!("/utxos/{addr}").as_str())
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create_async()
        .await;

    let client = RpcClient::new(server.url());
    let utxos  = client.get_utxos(addr).await.unwrap();

    assert_eq!(utxos.len(), 1);
    assert_eq!(utxos[0].amount, 500);
    assert_eq!(utxos[0].index, 0);
    mock.assert_async().await;
}

#[tokio::test]
async fn non_2xx_response_becomes_node_error() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/chain/tip")
        .with_status(503)
        .with_body("node shutting down")
        .create_async()
        .await;

    let client = RpcClient::new(server.url());
    let result = client.get_tip().await;

    assert!(matches!(
        result,
        Err(ClientError::NodeError { status: 503, .. })
    ));
    mock.assert_async().await;
}

#[tokio::test]
async fn bad_request_returns_node_error_with_body() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/balance/invalid")
        .with_status(400)
        .with_body("invalid address")
        .create_async()
        .await;

    let client = RpcClient::new(server.url());
    let result = client.get_balance("invalid").await;

    match result {
        Err(ClientError::NodeError { status, body }) => {
            assert_eq!(status, 400);
            assert_eq!(body, "invalid address");
        }
        other => panic!("expected NodeError, got {other:?}"),
    }
    mock.assert_async().await;
}

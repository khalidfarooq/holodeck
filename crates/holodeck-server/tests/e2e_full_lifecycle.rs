use holodeck_mcp::HolodeckMcpServer;
use holodeck_server::HolodeckApp;
use reqwest::Client;
use serde_json::json;

#[tokio::test]
async fn test_full_mock_cloud_e2e_lifecycle() {
    // 1. Bind to ephemeral port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind ephemeral TCP port");
    let addr = listener.local_addr().unwrap();
    let port = addr.port();

    let app = HolodeckApp::new(port);
    let router = app.build_router();
    let event_bus = app.event_bus.clone();

    // 2. Spawn Axum server background task
    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("Server failed");
    });

    let client = Client::new();
    let base_url = format!("http://127.0.0.1:{}", port);

    // 3. Verify Health Check
    let health_res = client
        .get(format!("{}/health", base_url))
        .send()
        .await
        .expect("Health check request failed");
    assert_eq!(health_res.status(), 200);
    let health_json: serde_json::Value = health_res.json().await.unwrap();
    assert_eq!(health_json["status"], "healthy");
    assert_eq!(health_json["service"], "holodeck");

    // 4. Test Stripe Customers and Checkout Sessions
    let cust_res = client
        .post(format!("{}/v1/customers", base_url))
        .json(&json!({
            "email": "agent@autonomous.dev",
            "name": "Autonomous Agent"
        }))
        .send()
        .await
        .expect("Create customer request failed");
    assert_eq!(cust_res.status(), 200);
    let cust_json: serde_json::Value = cust_res.json().await.unwrap();
    assert!(cust_json["id"].as_str().unwrap().starts_with("cus_"));

    let session_res = client
        .post(format!("{}/v1/checkout/sessions", base_url))
        .json(&json!({
            "customer": cust_json["id"],
            "amount_total": 5000,
            "currency": "usd"
        }))
        .send()
        .await
        .expect("Create checkout session failed");
    assert_eq!(session_res.status(), 200);
    let session_json: serde_json::Value = session_res.json().await.unwrap();
    let session_id = session_json["id"].as_str().unwrap();

    // Complete Checkout Session
    let complete_res = client
        .post(format!("{}/v1/checkout/sessions/{}/complete", base_url, session_id))
        .send()
        .await
        .expect("Complete checkout session failed");
    assert_eq!(complete_res.status(), 200);
    let complete_json: serde_json::Value = complete_res.json().await.unwrap();
    assert_eq!(complete_json["payment_status"], "paid");
    assert_eq!(complete_json["status"], "complete");

    // 5. Test S3 Storage Upload and Download
    let bucket = "agent-artifacts";
    let key = "deploy/report.txt";
    let artifact_payload = "Agent deployment status: ALL_TESTS_PASSED";

    let upload_res = client
        .put(format!("{}/storage/{}/{}", base_url, bucket, key))
        .body(artifact_payload)
        .send()
        .await
        .expect("S3 upload failed");
    assert_eq!(upload_res.status(), 200);

    let download_res = client
        .get(format!("{}/storage/{}/{}", base_url, bucket, key))
        .send()
        .await
        .expect("S3 download failed");
    assert_eq!(download_res.status(), 200);
    let download_body = download_res.text().await.unwrap();
    assert_eq!(download_body, artifact_payload);

    // 6. Test Notifications (Resend Email)
    let email_res = client
        .post(format!("{}/v1/emails", base_url))
        .json(&json!({
            "from": "deploy@company.com",
            "to": "dev@company.com",
            "subject": "Deploy Success",
            "text": "Production build passed"
        }))
        .send()
        .await
        .expect("Resend email failed");
    assert_eq!(email_res.status(), 200);
    let email_json: serde_json::Value = email_res.json().await.unwrap();
    assert!(email_json["id"].as_str().unwrap().starts_with("re_"));

    // 7. Test OAuth2 Token Exchange & UserInfo
    let token_res = client
        .post(format!("{}/oauth/token", base_url))
        .json(&json!({
            "grant_type": "authorization_code",
            "code": "test_auth_code_123",
            "client_id": "test_client",
            "client_secret": "test_secret"
        }))
        .send()
        .await
        .expect("OAuth token exchange failed");
    assert_eq!(token_res.status(), 200);
    let token_json: serde_json::Value = token_res.json().await.unwrap();
    let access_token = token_json["access_token"].as_str().unwrap();

    let userinfo_res = client
        .get(format!("{}/userinfo", base_url))
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await
        .expect("OAuth userinfo failed");
    assert_eq!(userinfo_res.status(), 200);
    let userinfo_json: serde_json::Value = userinfo_res.json().await.unwrap();
    assert_eq!(userinfo_json["email"], "developer@holodeck.local");

    // 8. Verify EventBus via HTTP /events endpoint
    let events_res = client
        .get(format!("{}/events", base_url))
        .send()
        .await
        .expect("List events failed");
    assert_eq!(events_res.status(), 200);
    let events_list: Vec<serde_json::Value> = events_res.json().await.unwrap();
    assert!(events_list.len() >= 4, "Expected at least 4 captured events");

    // 9. Verify Agent MCP Tool Integration
    let mcp = HolodeckMcpServer::new(event_bus.clone());

    // Call holodeck_inspect_events tool
    let mcp_events = mcp
        .handle_tool_call("holodeck_inspect_events", json!({"limit": 50}))
        .await
        .expect("MCP inspect events failed");
    assert!(mcp_events.as_array().unwrap().len() >= 4);

    // Call holodeck_get_mock_credentials tool
    let mcp_creds = mcp
        .handle_tool_call("holodeck_get_mock_credentials", json!({"provider": "s3"}))
        .await
        .expect("MCP credentials failed");
    assert_eq!(mcp_creds["provider"], "s3");

    // Call holodeck_reset_state tool
    let reset_res = mcp
        .handle_tool_call("holodeck_reset_state", json!({}))
        .await
        .expect("MCP reset state failed");
    assert_eq!(reset_res["status"], "cleared");

    // Verify events are cleared after reset
    let cleared_events = mcp
        .handle_tool_call("holodeck_inspect_events", json!({}))
        .await
        .expect("MCP inspect after reset failed");
    assert_eq!(cleared_events.as_array().unwrap().len(), 0);
}

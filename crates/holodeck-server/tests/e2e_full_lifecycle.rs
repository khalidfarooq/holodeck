use axum::{http::HeaderMap, routing::post, Router};
use holodeck_core::sign_stripe_webhook;
use holodeck_mcp::HolodeckMcpServer;
use holodeck_server::HolodeckApp;
use reqwest::{redirect::Policy, Client};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

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

    // 2. Spawn Axum server background task
    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("Server failed");
    });

    let client = Client::new();
    let no_redirect_client = Client::builder()
        .redirect(Policy::none())
        .build()
        .unwrap();
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

    // 4. Test Stripe Customers, Checkout Sessions (dynamic amount/currency), and validation
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
    let cust_id = cust_json["id"].as_str().unwrap().to_string();
    assert!(cust_id.starts_with("cus_"));

    let session_res = client
        .post(format!("{}/v1/checkout/sessions", base_url))
        .json(&json!({
            "customer": cust_id,
            "amount_total": 7500,
            "currency": "eur"
        }))
        .send()
        .await
        .expect("Create checkout session failed");
    assert_eq!(session_res.status(), 200);
    let session_json: serde_json::Value = session_res.json().await.unwrap();
    let session_id = session_json["id"].as_str().unwrap().to_string();
    assert_eq!(session_json["amount_total"], 7500);
    assert_eq!(session_json["currency"], "eur");

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

    // Invalid PaymentIntent amount returns Stripe-shaped error
    let bad_pi_res = client
        .post(format!("{}/v1/payment_intents", base_url))
        .json(&json!({
            "amount": 0,
            "currency": "usd"
        }))
        .send()
        .await
        .expect("Invalid payment intent request failed");
    assert_eq!(bad_pi_res.status(), 400);
    let bad_pi_json: serde_json::Value = bad_pi_res.json().await.unwrap();
    assert_eq!(bad_pi_json["error"]["type"], "invalid_request_error");
    assert_eq!(bad_pi_json["error"]["param"], "amount");

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

    // 6. Test Notifications (Resend & SendGrid personalizations)
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

    let sg_res = client
        .post(format!("{}/v3/mail/send", base_url))
        .json(&json!({
            "personalizations": [{
                "to": [{"email": "oncall@company.com"}]
            }],
            "from": {"email": "alerts@company.com"},
            "subject": "Release Alert",
            "content": [{"type": "text/plain", "value": "Deployed v1.0"}]
        }))
        .send()
        .await
        .expect("SendGrid email failed");
    assert_eq!(sg_res.status(), 202);

    let inbox_res: Vec<serde_json::Value> = client
        .get(format!("{}/inbox/emails", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(inbox_res.len(), 2);
    assert_eq!(inbox_res[1]["to"][0], "oncall@company.com");

    // 7. Test Full OAuth2 Authorize -> Token Exchange -> UserInfo Flow
    let unissued_token_res = client
        .post(format!("{}/oauth/token", base_url))
        .json(&json!({
            "grant_type": "authorization_code",
            "code": "test_auth_code_123"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(unissued_token_res.status(), 401);

    let auth_res = no_redirect_client
        .get(format!(
            "{}/oauth/authorize?client_id=test_client&redirect_uri=http://127.0.0.1:3000/callback&state=xyz123",
            base_url
        ))
        .send()
        .await
        .expect("OAuth authorize failed");
    assert_eq!(auth_res.status(), 303);
    let location = auth_res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(location.contains("state=xyz123"));
    let code = location
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_string();

    let token_res = client
        .post(format!("{}/oauth/token", base_url))
        .json(&json!({
            "grant_type": "authorization_code",
            "code": code,
            "client_id": "test_client",
            "client_secret": "test_secret"
        }))
        .send()
        .await
        .expect("OAuth token exchange failed");
    assert_eq!(token_res.status(), 200);
    let token_json: serde_json::Value = token_res.json().await.unwrap();
    let access_token = token_json["access_token"].as_str().unwrap().to_string();

    // Missing / invalid token on /userinfo -> 401
    let unauth_userinfo = client
        .get(format!("{}/userinfo", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth_userinfo.status(), 401);

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
    assert!(events_list.len() >= 8, "Expected at least 8 captured events");

    // 9. Verify Agent MCP Tool Integration connected ONLY via HTTP to the live daemon
    let mcp = HolodeckMcpServer::connect(&base_url);

    // Call holodeck_inspect_events tool over HTTP
    let mcp_events = mcp
        .handle_tool_call("holodeck_inspect_events", json!({"limit": 50}))
        .await
        .expect("MCP inspect events failed");
    assert!(mcp_events.as_array().unwrap().len() >= 8);

    // Spawn a local webhook receiver and trigger a signed webhook via MCP -> Daemon
    let received_sig = Arc::new(Mutex::new(None::<(String, Vec<u8>)>));
    let received_sig_clone = received_sig.clone();
    let webhook_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let webhook_port = webhook_listener.local_addr().unwrap().port();
    let webhook_app = Router::new().route(
        "/hook",
        post(move |headers: HeaderMap, body: axum::body::Bytes| {
            let store = received_sig_clone.clone();
            async move {
                let sig = headers
                    .get("Stripe-Signature")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                *store.lock().await = Some((sig, body.to_vec()));
                "ok"
            }
        }),
    );
    tokio::spawn(async move {
        axum::serve(webhook_listener, webhook_app).await.unwrap();
    });

    let trigger_res = mcp
        .handle_tool_call(
            "holodeck_trigger_webhook",
            json!({
                "provider": "stripe",
                "target_url": format!("http://127.0.0.1:{}/hook", webhook_port),
                "event_type": "checkout.session.completed"
            }),
        )
        .await
        .expect("MCP trigger webhook failed");
    assert_eq!(trigger_res["success"], true);
    assert_eq!(trigger_res["status_code"], 200);

    let captured_hook = received_sig.lock().await.clone().expect("Webhook not received");
    let ts: i64 = captured_hook
        .0
        .split(',')
        .find_map(|part| part.strip_prefix("t="))
        .expect("Missing Stripe-Signature timestamp")
        .parse()
        .expect("Invalid Stripe-Signature timestamp");
    let expected_sig =
        sign_stripe_webhook(&captured_hook.1, "whsec_test_holodeck_secret_key", ts).unwrap();
    assert_eq!(captured_hook.0, expected_sig);

    // Call holodeck_get_mock_credentials tool
    let mcp_creds = mcp
        .handle_tool_call("holodeck_get_mock_credentials", json!({"provider": "s3"}))
        .await
        .expect("MCP credentials failed");
    assert_eq!(mcp_creds["provider"], "s3");

    // Call holodeck_reset_state tool -> POST /reset on live daemon
    let reset_res = mcp
        .handle_tool_call("holodeck_reset_state", json!({}))
        .await
        .expect("MCP reset state failed");
    assert_eq!(reset_res["status"], "cleared");

    // Verify EventBus AND all provider stores are cleared after reset
    let cleared_events = mcp
        .handle_tool_call("holodeck_inspect_events", json!({}))
        .await
        .expect("MCP inspect after reset failed");
    assert_eq!(cleared_events.as_array().unwrap().len(), 0);

    assert_eq!(
        client
            .get(format!("{}/v1/customers/{}", base_url, cust_id))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .get(format!("{}/v1/checkout/sessions/{}", base_url, session_id))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .get(format!("{}/storage/{}/{}", base_url, bucket, key))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let cleared_inbox: Vec<serde_json::Value> = client
        .get(format!("{}/inbox/emails", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cleared_inbox.len(), 0);

    assert_eq!(
        client
            .get(format!("{}/userinfo", base_url))
            .header("Authorization", format!("Bearer {}", access_token))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}

#[tokio::test]
async fn test_provider_scoped_reset() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = HolodeckApp::new(port);
    let router = app.build_router();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let client = Client::new();
    let base_url = format!("http://127.0.0.1:{}", port);

    // Create Stripe customer and S3 object
    let cust: serde_json::Value = client
        .post(format!("{}/v1/customers", base_url))
        .json(&json!({"email": "keep-or-reset@example.com"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let cust_id = cust["id"].as_str().unwrap();

    client
        .put(format!("{}/storage/bucket/file.txt", base_url))
        .body("hello")
        .send()
        .await
        .unwrap();

    // Scoped reset for stripe only via query param
    let reset_stripe = client
        .post(format!("{}/reset?provider=stripe", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(reset_stripe.status(), 200);

    // Stripe customer is gone, S3 object and storage events remain
    assert_eq!(
        client
            .get(format!("{}/v1/customers/{}", base_url, cust_id))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .get(format!("{}/storage/bucket/file.txt", base_url))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    let remaining_events: Vec<serde_json::Value> = client
        .get(format!("{}/events", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(remaining_events.len(), 1);
    assert_eq!(remaining_events[0]["provider"], "storage");
}

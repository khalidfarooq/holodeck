use holodeck_core::credentials::get_mock_credentials;
use holodeck_core::dispatcher::WebhookDispatcher;
use holodeck_core::events::EventBus;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: Option<String>,
    pub id: Option<serde_json::Value>,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
}

pub struct HolodeckMcpServer {
    pub event_bus: EventBus,
    pub dispatcher: WebhookDispatcher,
    pub daemon_url: Option<String>,
    pub http_client: reqwest::Client,
}

impl HolodeckMcpServer {
    /// Creates a standalone in-memory MCP server backed by the provided EventBus.
    pub fn new(event_bus: EventBus) -> Self {
        let dispatcher = WebhookDispatcher::new(event_bus.clone());
        Self {
            event_bus,
            dispatcher,
            daemon_url: None,
            http_client: reqwest::Client::new(),
        }
    }

    /// Creates an MCP server connected to a running Holodeck HTTP daemon.
    pub fn connect(base_url: impl Into<String>) -> Self {
        Self::new(EventBus::new()).with_daemon_url(base_url)
    }

    /// Configures this MCP server to proxy state inspection, webhook dispatch, and reset calls
    /// to a running Holodeck HTTP daemon at `base_url`.
    pub fn with_daemon_url(mut self, base_url: impl Into<String>) -> Self {
        let normalized = base_url.into().trim_end_matches('/').to_string();
        self.daemon_url = Some(normalized);
        self
    }

    async fn parse_daemon_response(
        &self,
        resp: reqwest::Response,
        endpoint: &str,
    ) -> Result<serde_json::Value, String> {
        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            format!(
                "Failed reading response from Holodeck daemon {}: {}",
                endpoint, e
            )
        })?;

        if !status.is_success() {
            return Err(format!(
                "Holodeck daemon {} returned HTTP {}: {}",
                endpoint, status, body
            ));
        }

        if body.trim().is_empty() {
            return Ok(serde_json::json!({}));
        }

        serde_json::from_str(&body).map_err(|e| {
            format!(
                "Invalid JSON response from Holodeck daemon {}: {}",
                endpoint, e
            )
        })
    }

    fn unreachable_error(&self, base_url: &str, err: reqwest::Error) -> String {
        format!(
            "Unable to reach Holodeck daemon at {} ({}). Start it with `holodeck start` or run MCP with `--standalone`.",
            base_url, err
        )
    }

    /// Handles tool invocations from AI coding agents (Claude Code, Antigravity, Cursor)
    pub async fn handle_tool_call(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match name {
            "holodeck_trigger_webhook" => {
                let provider = args.get("provider").and_then(|v| v.as_str()).unwrap_or("stripe");
                let target_url = args
                    .get("target_url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "Missing required parameter: target_url".to_string())?;
                let event_type = args
                    .get("event_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("checkout.session.completed");
                let secret = args
                    .get("secret")
                    .and_then(|v| v.as_str())
                    .unwrap_or("whsec_test_holodeck_secret_key");

                let default_payload = serde_json::json!({
                    "id": format!("evt_test_{}", uuid::Uuid::new_v4().simple()),
                    "object": "event",
                    "type": event_type,
                    "created": chrono::Utc::now().timestamp(),
                    "data": {
                        "object": {
                            "id": format!("cs_test_{}", uuid::Uuid::new_v4().simple()),
                            "object": "checkout.session",
                            "payment_status": "paid",
                            "status": "complete"
                        }
                    }
                });

                let payload = args.get("payload").unwrap_or(&default_payload);

                if let Some(ref base_url) = self.daemon_url {
                    let endpoint = format!("{}/webhook/trigger", base_url);
                    let resp = self
                        .http_client
                        .post(&endpoint)
                        .json(&serde_json::json!({
                            "provider": provider,
                            "target_url": target_url,
                            "event_type": event_type,
                            "payload": payload,
                            "secret": secret,
                        }))
                        .send()
                        .await
                        .map_err(|e| self.unreachable_error(base_url, e))?;

                    self.parse_daemon_response(resp, &endpoint).await
                } else {
                    let result = self
                        .dispatcher
                        .dispatch(provider, target_url, event_type, payload, secret)
                        .await?;

                    serde_json::to_value(result).map_err(|e| e.to_string())
                }
            }
            "holodeck_inspect_events" => {
                let provider = args.get("provider").and_then(|v| v.as_str());
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

                if let Some(ref base_url) = self.daemon_url {
                    let endpoint = format!("{}/events", base_url);
                    let mut req = self
                        .http_client
                        .get(&endpoint)
                        .query(&[("limit", limit.to_string())]);
                    if let Some(p) = provider {
                        req = req.query(&[("provider", p)]);
                    }

                    let resp = req
                        .send()
                        .await
                        .map_err(|e| self.unreachable_error(base_url, e))?;

                    self.parse_daemon_response(resp, &endpoint).await
                } else {
                    let events = self.event_bus.list(provider, limit).await;
                    serde_json::to_value(events).map_err(|e| e.to_string())
                }
            }
            "holodeck_get_mock_credentials" => {
                let provider = args.get("provider").and_then(|v| v.as_str()).unwrap_or("stripe");
                let creds = get_mock_credentials(provider);
                serde_json::to_value(creds).map_err(|e| e.to_string())
            }
            "holodeck_reset_state" => {
                let provider = args.get("provider").and_then(|v| v.as_str());

                if let Some(ref base_url) = self.daemon_url {
                    let endpoint = format!("{}/reset", base_url);
                    let resp = self
                        .http_client
                        .post(&endpoint)
                        .json(&serde_json::json!({ "provider": provider }))
                        .send()
                        .await
                        .map_err(|e| self.unreachable_error(base_url, e))?;

                    self.parse_daemon_response(resp, &endpoint).await
                } else {
                    self.event_bus.clear(provider).await;
                    Ok(serde_json::json!({"status": "cleared"}))
                }
            }
            _ => Err(format!("Unknown tool: {}", name)),
        }
    }

    /// Runs the Model Context Protocol stdio loop
    pub async fn run_stdio(self: Arc<Self>) -> Result<(), Box<dyn std::error::Error>> {
        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let mut lines = BufReader::new(stdin).lines();

        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }

            let req: JsonRpcRequest = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    let err_resp = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: None,
                        result: None,
                        error: Some(serde_json::json!({
                            "code": -32700,
                            "message": format!("Parse error: {}", e)
                        })),
                    };
                    let out = serde_json::to_string(&err_resp)? + "\n";
                    stdout.write_all(out.as_bytes()).await?;
                    stdout.flush().await?;
                    continue;
                }
            };

            let resp = match req.method.as_str() {
                "initialize" => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result: Some(serde_json::json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": {
                            "tools": {}
                        },
                        "serverInfo": {
                            "name": "holodeck-mcp",
                            "version": "0.1.0"
                        }
                    })),
                    error: None,
                },
                "notifications/initialized" => {
                    continue;
                }
                "tools/list" => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result: Some(serde_json::json!({
                        "tools": [
                            {
                                "name": "holodeck_trigger_webhook",
                                "description": "Dispatches a cryptographic HMAC-SHA256 signed webhook payload to your local dev server (e.g. Stripe, GitHub).",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "provider": {
                                            "type": "string",
                                            "enum": ["stripe", "github"],
                                            "default": "stripe"
                                        },
                                        "target_url": {
                                            "type": "string",
                                            "description": "Local webhook handler URL (e.g. 'http://localhost:3000/api/webhooks/stripe')"
                                        },
                                        "event_type": {
                                            "type": "string",
                                            "description": "Event type (e.g. 'checkout.session.completed', 'invoice.payment_succeeded')"
                                        },
                                        "payload": {
                                            "type": "object",
                                            "description": "Optional custom event payload dictionary"
                                        },
                                        "secret": {
                                            "type": "string",
                                            "description": "Optional webhook secret (defaults to 'whsec_test_holodeck_secret_key')"
                                        }
                                    },
                                    "required": ["target_url"]
                                }
                            },
                            {
                                "name": "holodeck_inspect_events",
                                "description": "Queries captured emails, SMS, charges, and dispatched webhooks to verify backend side-effects during tests.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "provider": {
                                            "type": "string",
                                            "description": "Optional provider filter ('stripe', 'email', 'sms', 'oauth', 'storage')"
                                        },
                                        "limit": {
                                            "type": "integer",
                                            "default": 20
                                        }
                                    }
                                }
                            },
                            {
                                "name": "holodeck_get_mock_credentials",
                                "description": "Returns mock API keys, endpoints, and .env.test configuration snippets for any cloud provider.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "provider": {
                                            "type": "string",
                                            "enum": ["stripe", "oauth", "s3", "resend"],
                                            "default": "stripe"
                                        }
                                    },
                                    "required": ["provider"]
                                }
                            },
                            {
                                "name": "holodeck_reset_state",
                                "description": "Resets captured event logs and mock databases between test runs.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "provider": {
                                            "type": "string",
                                            "description": "Optional provider to clear"
                                        }
                                    }
                                }
                            }
                        ]
                    })),
                    error: None,
                },
                "tools/call" => {
                    let tool_name = req
                        .params
                        .as_ref()
                        .and_then(|p| p.get("name"))
                        .and_then(|n| n.as_str())
                        .unwrap_or("");
                    let arguments = req
                        .params
                        .as_ref()
                        .and_then(|p| p.get("arguments"))
                        .cloned()
                        .unwrap_or(serde_json::json!({}));

                    match self.handle_tool_call(tool_name, arguments).await {
                        Ok(data) => JsonRpcResponse {
                            jsonrpc: "2.0".to_string(),
                            id: req.id,
                            result: Some(serde_json::json!({
                                "content": [
                                    {
                                        "type": "text",
                                        "text": serde_json::to_string_pretty(&data).unwrap_or_default()
                                    }
                                ]
                            })),
                            error: None,
                        },
                        Err(e) => JsonRpcResponse {
                            jsonrpc: "2.0".to_string(),
                            id: req.id,
                            result: Some(serde_json::json!({
                                "isError": true,
                                "content": [
                                    {
                                        "type": "text",
                                        "text": format!("Tool error: {}", e)
                                    }
                                ]
                            })),
                            error: None,
                        },
                    }
                }
                _ => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result: None,
                    error: Some(serde_json::json!({
                        "code": -32601,
                        "message": format!("Method not found: {}", req.method)
                    })),
                },
            };

            let out = serde_json::to_string(&resp)? + "\n";
            stdout.write_all(out.as_bytes()).await?;
            stdout.flush().await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mcp_tool_handling() {
        let bus = EventBus::new();
        let server = HolodeckMcpServer::new(bus.clone());

        // 1. Get mock credentials
        let creds_res = server
            .handle_tool_call(
                "holodeck_get_mock_credentials",
                serde_json::json!({"provider": "stripe"}),
            )
            .await
            .unwrap();
        assert_eq!(creds_res["provider"], "stripe");

        // 2. Emit an event and inspect via MCP
        bus.emit("stripe", "charge.captured", serde_json::json!({"amount": 100})).await;

        let events_res = server
            .handle_tool_call("holodeck_inspect_events", serde_json::json!({"limit": 5}))
            .await
            .unwrap();
        assert_eq!(events_res.as_array().unwrap().len(), 1);

        // 3. Reset state
        let reset_res = server
            .handle_tool_call("holodeck_reset_state", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(reset_res["status"], "cleared");
    }

    #[tokio::test]
    async fn test_mcp_daemon_unreachable_returns_clear_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let url = format!("http://127.0.0.1:{}/", port);
        let server = HolodeckMcpServer::connect(url);
        assert_eq!(
            server.daemon_url.as_deref(),
            Some(format!("http://127.0.0.1:{}", port).as_str())
        );

        let inspect_err = server
            .handle_tool_call("holodeck_inspect_events", serde_json::json!({"limit": 10}))
            .await
            .unwrap_err();
        assert!(inspect_err.contains("Unable to reach Holodeck daemon"));
        assert!(inspect_err.contains("holodeck start"));

        let reset_err = server
            .handle_tool_call("holodeck_reset_state", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(reset_err.contains("Unable to reach Holodeck daemon"));

        let trigger_err = server
            .handle_tool_call(
                "holodeck_trigger_webhook",
                serde_json::json!({"target_url": "http://127.0.0.1:9999/webhook"}),
            )
            .await
            .unwrap_err();
        assert!(trigger_err.contains("Unable to reach Holodeck daemon"));
    }
}

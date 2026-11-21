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
}

impl HolodeckMcpServer {
    pub fn new(event_bus: EventBus) -> Self {
        let dispatcher = WebhookDispatcher::new(event_bus.clone());
        Self {
            event_bus,
            dispatcher,
        }
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

                let result = self
                    .dispatcher
                    .dispatch(provider, target_url, event_type, payload, secret)
                    .await?;

                Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
            }
            "holodeck_inspect_events" => {
                let provider = args.get("provider").and_then(|v| v.as_str());
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

                let events = self.event_bus.list(provider, limit).await;
                Ok(serde_json::to_value(events).map_err(|e| e.to_string())?)
            }
            "holodeck_get_mock_credentials" => {
                let provider = args.get("provider").and_then(|v| v.as_str()).unwrap_or("stripe");
                let creds = get_mock_credentials(provider);
                Ok(serde_json::to_value(creds).map_err(|e| e.to_string())?)
            }
            "holodeck_reset_state" => {
                let provider = args.get("provider").and_then(|v| v.as_str());
                self.event_bus.clear(provider).await;
                Ok(serde_json::json!({"status": "cleared"}))
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
}

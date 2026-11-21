use crate::crypto::{sign_github_webhook, sign_stripe_webhook};
use crate::events::EventBus;
use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookDeliveryResult {
    pub success: bool,
    pub status_code: Option<u16>,
    pub response_body: String,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct WebhookDispatcher {
    client: reqwest::Client,
    event_bus: EventBus,
}

impl WebhookDispatcher {
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            client: reqwest::Client::new(),
            event_bus,
        }
    }

    /// Dispatches a signed webhook payload to a target URL
    pub async fn dispatch(
        &self,
        provider: &str,
        target_url: &str,
        event_type: &str,
        payload: &serde_json::Value,
        secret: &str,
    ) -> Result<WebhookDeliveryResult, String> {
        let payload_bytes = serde_json::to_vec(payload).map_err(|e| e.to_string())?;

        let mut request = self
            .client
            .post(target_url)
            .header("Content-Type", "application/json");

        match provider.to_lowercase().as_str() {
            "stripe" => {
                let timestamp = Utc::now().timestamp();
                let sig = sign_stripe_webhook(&payload_bytes, secret, timestamp)?;
                request = request.header("Stripe-Signature", sig);
            }
            "github" => {
                let sig = sign_github_webhook(&payload_bytes, secret)?;
                request = request
                    .header("X-Hub-Signature-256", sig)
                    .header("X-GitHub-Event", event_type);
            }
            _ => {
                // generic webhook
                request = request.header("X-Webhook-Event", event_type);
            }
        }

        // Record in event bus
        self.event_bus
            .emit(
                provider,
                &format!("webhook.dispatched.{}", event_type),
                serde_json::json!({
                    "target_url": target_url,
                    "payload": payload,
                }),
            )
            .await;

        match request.body(payload_bytes).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                Ok(WebhookDeliveryResult {
                    success: status >= 200 && status < 300,
                    status_code: Some(status),
                    response_body: body,
                    error: None,
                })
            }
            Err(e) => Ok(WebhookDeliveryResult {
                success: false,
                status_code: None,
                response_body: "".to_string(),
                error: Some(e.to_string()),
            }),
        }
    }
}

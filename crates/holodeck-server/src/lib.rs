use axum::{
    body::Bytes,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use holodeck_core::dispatcher::{WebhookDeliveryResult, WebhookDispatcher};
use holodeck_core::events::{EventBus, MockEvent};
use holodeck_notif::NotificationSimulator;
use holodeck_oauth::OAuthSimulator;
use holodeck_storage::StorageSimulator;
use holodeck_stripe::StripeSimulator;
use serde::Deserialize;
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

#[derive(Clone)]
pub struct HolodeckApp {
    pub event_bus: EventBus,
    pub dispatcher: WebhookDispatcher,
    pub base_url: String,
    pub stripe: StripeSimulator,
    pub oauth: OAuthSimulator,
    pub storage: StorageSimulator,
    pub notif: NotificationSimulator,
}

impl HolodeckApp {
    pub fn new(port: u16) -> Self {
        let base_url = format!("http://127.0.0.1:{}", port);
        let event_bus = EventBus::new();
        let dispatcher = WebhookDispatcher::new(event_bus.clone());
        let stripe = StripeSimulator::new(event_bus.clone(), base_url.clone());
        let oauth = OAuthSimulator::new(event_bus.clone());
        let storage = StorageSimulator::new(event_bus.clone(), base_url.clone());
        let notif = NotificationSimulator::new(event_bus.clone());

        Self {
            event_bus,
            dispatcher,
            base_url,
            stripe,
            oauth,
            storage,
            notif,
        }
    }

    pub fn build_router(&self) -> Router {
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        Router::new()
            .route("/health", get(health_check))
            .route("/events", get(list_events).delete(clear_events))
            .route("/reset", post(reset_state))
            .route("/webhook/trigger", post(trigger_webhook))
            .with_state(self.clone())
            .merge(self.stripe.clone().router())
            .merge(self.oauth.clone().router())
            .merge(self.storage.clone().router())
            .merge(self.notif.clone().router())
            .layer(cors)
    }

    pub async fn run(&self, port: u16) -> Result<(), Box<dyn std::error::Error>> {
        let app = self.build_router();
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        info!("🌌 Holodeck Mock Cloud Server listening on http://{}", addr);

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;
        Ok(())
    }
}

async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "service": "holodeck",
        "version": "0.1.0"
    }))
}

#[derive(Deserialize)]
pub struct EventQueryParams {
    pub provider: Option<String>,
    pub limit: Option<usize>,
}

async fn list_events(
    State(app): State<HolodeckApp>,
    Query(params): Query<EventQueryParams>,
) -> Json<Vec<MockEvent>> {
    let limit = params.limit.unwrap_or(50);
    let events = app.event_bus.list(params.provider.as_deref(), limit).await;
    Json(events)
}

async fn clear_events(
    State(app): State<HolodeckApp>,
    Query(params): Query<EventQueryParams>,
) -> impl IntoResponse {
    app.event_bus.clear(params.provider.as_deref()).await;
    StatusCode::NO_CONTENT
}

#[derive(Deserialize, Default)]
pub struct ResetParams {
    pub provider: Option<String>,
}

async fn reset_state(
    State(app): State<HolodeckApp>,
    Query(query): Query<ResetParams>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let body_provider = if body.is_empty() {
        None
    } else {
        let parsed: ResetParams = serde_json::from_slice(&body)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid JSON body: {}", e)))?;
        parsed.provider
    };

    let provider = query.provider.or(body_provider);

    match provider.as_deref().map(|p| p.to_lowercase()).as_deref() {
        Some("stripe") => {
            app.stripe.reset().await;
            app.event_bus.clear(Some("stripe")).await;
        }
        Some("oauth" | "google" | "github") => {
            app.oauth.reset().await;
            app.event_bus.clear(Some("oauth")).await;
        }
        Some("storage" | "s3" | "aws") => {
            app.storage.reset().await;
            app.event_bus.clear(Some("storage")).await;
        }
        Some("email" | "resend" | "sendgrid") => {
            app.notif.clear_emails().await;
            app.event_bus.clear(Some("email")).await;
        }
        Some("sms" | "twilio") => {
            app.notif.clear_sms().await;
            app.event_bus.clear(Some("sms")).await;
        }
        Some("notif" | "notifications") => {
            app.notif.reset().await;
            app.event_bus.clear(Some("email")).await;
            app.event_bus.clear(Some("sms")).await;
        }
        Some(other) => {
            app.event_bus.clear(Some(other)).await;
        }
        None => {
            app.stripe.reset().await;
            app.oauth.reset().await;
            app.storage.reset().await;
            app.notif.reset().await;
            app.event_bus.clear(None).await;
        }
    }

    Ok(Json(serde_json::json!({
        "status": "cleared",
        "provider": provider
    })))
}

#[derive(Deserialize)]
pub struct TriggerWebhookPayload {
    pub provider: String,
    pub target_url: String,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub secret: Option<String>,
}

async fn trigger_webhook(
    State(app): State<HolodeckApp>,
    Json(body): Json<TriggerWebhookPayload>,
) -> Result<Json<WebhookDeliveryResult>, (StatusCode, String)> {
    let secret = body
        .secret
        .unwrap_or_else(|| "whsec_test_holodeck_secret_key".to_string());

    match app
        .dispatcher
        .dispatch(
            &body.provider,
            &body.target_url,
            &body.event_type,
            &body.payload,
            &secret,
        )
        .await
    {
        Ok(result) => Ok(Json(result)),
        Err(err) => Err((StatusCode::INTERNAL_SERVER_ERROR, err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_holodeck_app_builder() {
        let app = HolodeckApp::new(8787);
        let _router = app.build_router();

        let events = app.event_bus.list(None, 10).await;
        assert_eq!(events.len(), 0);
    }
}

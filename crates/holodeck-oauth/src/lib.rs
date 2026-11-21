use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::{get, post},
    Form, Json, Router,
};
use holodeck_core::events::EventBus;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCode {
    pub code: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scope: String,
    pub state: Option<String>,
}

#[derive(Default)]
pub struct OAuthDbState {
    pub codes: HashMap<String, OAuthCode>,
    pub tokens: HashMap<String, String>, // token -> user_id
}

#[derive(Clone)]
pub struct OAuthSimulator {
    pub state: Arc<RwLock<OAuthDbState>>,
    pub event_bus: EventBus,
}

impl OAuthSimulator {
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            state: Arc::new(RwLock::new(OAuthDbState::default())),
            event_bus,
        }
    }

    pub fn router(self) -> Router {
        Router::new()
            .route("/oauth/authorize", get(authorize))
            .route("/oauth/token", post(token_exchange))
            .route("/userinfo", get(userinfo))
            .route("/user", get(userinfo))
            .with_state(self)
    }
}

#[derive(Deserialize)]
pub struct AuthorizeParams {
    pub client_id: String,
    pub redirect_uri: String,
    pub scope: Option<String>,
    pub state: Option<String>,
}

async fn authorize(
    State(sim): State<OAuthSimulator>,
    Query(params): Query<AuthorizeParams>,
) -> impl IntoResponse {
    let code_str = format!("auth_code_{}", Uuid::new_v4().simple());
    let code_entry = OAuthCode {
        code: code_str.clone(),
        client_id: params.client_id,
        redirect_uri: params.redirect_uri.clone(),
        scope: params.scope.unwrap_or_else(|| "openid profile email".to_string()),
        state: params.state.clone(),
    };

    let mut lock = sim.state.write().await;
    lock.codes.insert(code_str.clone(), code_entry);

    sim.event_bus
        .emit("oauth", "oauth.authorized", serde_json::json!({"code": code_str}))
        .await;

    let mut redirect_url = if params.redirect_uri.contains('?') {
        format!("{}&code={}", params.redirect_uri, code_str)
    } else {
        format!("{}?code={}", params.redirect_uri, code_str)
    };

    if let Some(state) = params.state {
        redirect_url.push_str(&format!("&state={}", state));
    }

    Redirect::to(&redirect_url)
}

#[derive(Deserialize)]
pub struct TokenExchangePayload {
    pub code: Option<String>,
    pub grant_type: Option<String>,
}

async fn token_exchange(
    State(sim): State<OAuthSimulator>,
    Form(payload): Form<TokenExchangePayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let code = payload.code.ok_or(StatusCode::BAD_REQUEST)?;

    let mut lock = sim.state.write().await;
    if lock.codes.remove(&code).is_some() {
        let token = format!("mock_token_{}", Uuid::new_v4().simple());
        lock.tokens.insert(token.clone(), "user_holodeck_123".to_string());

        sim.event_bus
            .emit("oauth", "oauth.token_exchanged", serde_json::json!({"token": token}))
            .await;

        Ok(Json(serde_json::json!({
            "access_token": token,
            "token_type": "Bearer",
            "expires_in": 3600,
            "id_token": "mock.jwt.id_token",
            "scope": "openid profile email"
        })))
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

async fn userinfo() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "sub": "user_holodeck_123",
        "id": "user_holodeck_123",
        "email": "developer@holodeck.local",
        "email_verified": true,
        "name": "Holodeck Test Developer",
        "login": "holodeck-user",
        "avatar_url": "https://holodeck.local/avatar.png",
        "picture": "https://holodeck.local/avatar.png"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_oauth_flow() {
        let bus = EventBus::new();
        let sim = OAuthSimulator::new(bus.clone());

        // 1. Authorize
        let auth_params = AuthorizeParams {
            client_id: "test_client".to_string(),
            redirect_uri: "http://localhost:3000/api/auth/callback".to_string(),
            scope: Some("openid email".to_string()),
            state: Some("xyz123".to_string()),
        };

        let response = authorize(State(sim.clone()), Query(auth_params)).await.into_response();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let code = {
            let lock = sim.state.read().await;
            lock.codes.keys().next().unwrap().clone()
        };

        // 2. Token Exchange
        let token_payload = TokenExchangePayload {
            code: Some(code),
            grant_type: Some("authorization_code".to_string()),
        };

        let Json(token_res) = token_exchange(State(sim.clone()), Form(token_payload)).await.unwrap();
        assert!(token_res["access_token"].is_string());

        // 3. Userinfo
        let Json(profile) = userinfo().await;
        assert_eq!(profile["email"], "developer@holodeck.local");

        // 4. Verify Events
        let events = bus.list(Some("oauth"), 10).await;
        assert_eq!(events.len(), 2);
    }
}

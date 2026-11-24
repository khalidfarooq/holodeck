use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    routing::{get, post},
    Json, Router,
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

    pub async fn reset(&self) {
        let mut lock = self.state.write().await;
        lock.codes.clear();
        lock.tokens.clear();
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenExchangePayload {
    pub code: Option<String>,
    pub grant_type: Option<String>,
}

async fn token_exchange(
    State(sim): State<OAuthSimulator>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let is_json = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("application/json"))
        .unwrap_or(false);

    let payload: TokenExchangePayload = if is_json {
        serde_json::from_slice(&body).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_request",
                    "error_description": "Malformed JSON request body"
                })),
            )
        })?
    } else {
        serde_urlencoded::from_bytes(&body).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_request",
                    "error_description": "Malformed form-urlencoded request body"
                })),
            )
        })?
    };

    let code = payload.code.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_request",
                "error_description": "Missing required parameter: code"
            })),
        )
    })?;

    let mut lock = sim.state.write().await;
    let issued_code = lock.codes.remove(&code);

    if let Some(code_entry) = issued_code {
        let token = format!("mock_token_{}", Uuid::new_v4().simple());
        lock.tokens
            .insert(token.clone(), "user_holodeck_123".to_string());
        drop(lock);

        sim.event_bus
            .emit(
                "oauth",
                "oauth.token_exchanged",
                serde_json::json!({"token": token}),
            )
            .await;

        Ok(Json(serde_json::json!({
            "access_token": token,
            "token_type": "Bearer",
            "expires_in": 3600,
            "id_token": "mock.jwt.id_token",
            "scope": code_entry.scope
        })))
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "Invalid or expired authorization code"
            })),
        ))
    }
}

fn extract_bearer_token(headers: &HeaderMap) -> Option<&str> {
    let header_val = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let mut parts = header_val.splitn(2, ' ');
    let scheme = parts.next()?;
    let token = parts.next()?.trim();
    if scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() {
        Some(token)
    } else {
        None
    }
}

async fn userinfo(
    State(sim): State<OAuthSimulator>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let token = extract_bearer_token(&headers).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "invalid_token",
                "error_description": "Missing or malformed Authorization Bearer token"
            })),
        )
    })?;

    let lock = sim.state.read().await;
    let user_id = lock.tokens.get(token).cloned().ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "invalid_token",
                "error_description": "Access token is invalid or expired"
            })),
        )
    })?;

    Ok(Json(serde_json::json!({
        "sub": user_id,
        "id": user_id,
        "email": "developer@holodeck.local",
        "email_verified": true,
        "name": "Holodeck Test Developer",
        "login": "holodeck-user",
        "avatar_url": "https://holodeck.local/avatar.png",
        "picture": "https://holodeck.local/avatar.png"
    })))
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

        let response = authorize(State(sim.clone()), Query(auth_params))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        let code = {
            let lock = sim.state.read().await;
            lock.codes.keys().next().unwrap().clone()
        };

        // 2. Unissued code must fail with 401
        let unissued_payload = TokenExchangePayload {
            code: Some("test_unissued_code".to_string()),
            grant_type: Some("authorization_code".to_string()),
        };
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "application/json".parse().unwrap());
        let bad_body = Bytes::from(serde_json::to_vec(&unissued_payload).unwrap());
        let (err_status, _) = token_exchange(State(sim.clone()), headers.clone(), bad_body)
            .await
            .unwrap_err();
        assert_eq!(err_status, StatusCode::UNAUTHORIZED);

        // 3. Token Exchange with issued code succeeds once
        let token_payload = TokenExchangePayload {
            code: Some(code.clone()),
            grant_type: Some("authorization_code".to_string()),
        };
        let body = Bytes::from(serde_json::to_vec(&token_payload).unwrap());

        let Json(token_res) = token_exchange(State(sim.clone()), headers.clone(), body.clone())
            .await
            .unwrap();
        let access_token = token_res["access_token"].as_str().unwrap().to_string();
        assert_eq!(token_res["scope"], "openid email");

        // Reusing same code must fail with 401
        let (reuse_status, _) = token_exchange(State(sim.clone()), headers, body)
            .await
            .unwrap_err();
        assert_eq!(reuse_status, StatusCode::UNAUTHORIZED);

        // 4. Userinfo rejects missing or invalid token
        let (no_auth_status, _) = userinfo(State(sim.clone()), HeaderMap::new())
            .await
            .unwrap_err();
        assert_eq!(no_auth_status, StatusCode::UNAUTHORIZED);

        let mut bad_auth = HeaderMap::new();
        bad_auth.insert("authorization", "Bearer invalid_tok".parse().unwrap());
        let (bad_auth_status, _) = userinfo(State(sim.clone()), bad_auth)
            .await
            .unwrap_err();
        assert_eq!(bad_auth_status, StatusCode::UNAUTHORIZED);

        // 5. Userinfo succeeds with issued Bearer token
        let mut auth_headers = HeaderMap::new();
        auth_headers.insert(
            "authorization",
            format!("Bearer {}", access_token).parse().unwrap(),
        );
        let Json(profile) = userinfo(State(sim.clone()), auth_headers.clone())
            .await
            .unwrap();
        assert_eq!(profile["email"], "developer@holodeck.local");
        assert_eq!(profile["sub"], "user_holodeck_123");

        // 6. Verify Events
        let events = bus.list(Some("oauth"), 10).await;
        assert_eq!(events.len(), 2);

        // 7. Reset invalidates tokens
        sim.reset().await;
        let (after_reset_status, _) = userinfo(State(sim.clone()), auth_headers)
            .await
            .unwrap_err();
        assert_eq!(after_reset_status, StatusCode::UNAUTHORIZED);
    }
}

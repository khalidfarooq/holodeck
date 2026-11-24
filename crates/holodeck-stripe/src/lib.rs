use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use holodeck_core::events::EventBus;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StripeCustomer {
    pub id: String,
    pub object: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub created: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StripePaymentIntent {
    pub id: String,
    pub object: String,
    pub amount: i64,
    pub currency: String,
    pub customer: Option<String>,
    pub status: String,
    pub client_secret: String,
    pub created: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StripeCheckoutSession {
    pub id: String,
    pub object: String,
    pub customer: Option<String>,
    pub amount_total: Option<i64>,
    pub currency: Option<String>,
    pub status: String,
    pub payment_status: String,
    pub url: String,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
    pub created: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StripeSubscription {
    pub id: String,
    pub object: String,
    pub customer: String,
    pub status: String,
    pub current_period_end: i64,
    pub created: i64,
}

#[derive(Default)]
pub struct StripeDbState {
    pub customers: HashMap<String, StripeCustomer>,
    pub payment_intents: HashMap<String, StripePaymentIntent>,
    pub checkout_sessions: HashMap<String, StripeCheckoutSession>,
    pub subscriptions: HashMap<String, StripeSubscription>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StripeErrorDetail {
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StripeErrorResponse {
    pub error: StripeErrorDetail,
}

fn stripe_error(
    status: StatusCode,
    error_type: &str,
    code: Option<&str>,
    param: Option<&str>,
    message: impl Into<String>,
) -> (StatusCode, Json<StripeErrorResponse>) {
    (
        status,
        Json(StripeErrorResponse {
            error: StripeErrorDetail {
                error_type: error_type.to_string(),
                code: code.map(|s| s.to_string()),
                message: message.into(),
                param: param.map(|s| s.to_string()),
            },
        }),
    )
}

#[derive(Clone)]
pub struct StripeSimulator {
    pub state: Arc<RwLock<StripeDbState>>,
    pub event_bus: EventBus,
    pub base_url: String,
}

impl StripeSimulator {
    pub fn new(event_bus: EventBus, base_url: String) -> Self {
        Self {
            state: Arc::new(RwLock::new(StripeDbState::default())),
            event_bus,
            base_url,
        }
    }

    pub async fn reset(&self) {
        let mut lock = self.state.write().await;
        lock.customers.clear();
        lock.payment_intents.clear();
        lock.checkout_sessions.clear();
        lock.subscriptions.clear();
    }

    pub fn router(self) -> Router {
        Router::new()
            .route("/v1/customers", post(create_customer).get(list_customers))
            .route("/v1/customers/:id", get(get_customer))
            .route("/v1/payment_intents", post(create_payment_intent))
            .route("/v1/payment_intents/:id", get(get_payment_intent))
            .route("/v1/checkout/sessions", post(create_checkout_session))
            .route("/v1/checkout/sessions/:id", get(get_checkout_session))
            .route("/v1/checkout/sessions/:id/complete", post(complete_checkout_session))
            .route("/v1/subscriptions", post(create_subscription))
            .route("/v1/subscriptions/:id", get(get_subscription).delete(cancel_subscription))
            .with_state(self)
    }
}

// Handlers
#[derive(Deserialize)]
pub struct CreateCustomerPayload {
    pub email: Option<String>,
    pub name: Option<String>,
}

async fn create_customer(
    State(sim): State<StripeSimulator>,
    Json(payload): Json<CreateCustomerPayload>,
) -> (StatusCode, Json<StripeCustomer>) {
    let id = format!("cus_test_{}", Uuid::new_v4().simple());
    let customer = StripeCustomer {
        id: id.clone(),
        object: "customer".to_string(),
        email: payload.email,
        name: payload.name,
        created: Utc::now().timestamp(),
    };

    let mut lock = sim.state.write().await;
    lock.customers.insert(id.clone(), customer.clone());

    sim.event_bus
        .emit("stripe", "customer.created", serde_json::to_value(&customer).unwrap())
        .await;

    (StatusCode::OK, Json(customer))
}

async fn list_customers(State(sim): State<StripeSimulator>) -> impl IntoResponse {
    let lock = sim.state.read().await;
    let list: Vec<StripeCustomer> = lock.customers.values().cloned().collect();
    Json(serde_json::json!({
        "object": "list",
        "data": list,
        "has_more": false
    }))
}

async fn get_customer(
    State(sim): State<StripeSimulator>,
    Path(id): Path<String>,
) -> Result<Json<StripeCustomer>, (StatusCode, Json<StripeErrorResponse>)> {
    let lock = sim.state.read().await;
    lock.customers.get(&id).cloned().map(Json).ok_or_else(|| {
        stripe_error(
            StatusCode::NOT_FOUND,
            "invalid_request_error",
            Some("resource_missing"),
            Some("id"),
            format!("No such customer: '{}'", id),
        )
    })
}

#[derive(Deserialize)]
pub struct CreatePaymentIntentPayload {
    pub amount: i64,
    pub currency: String,
    pub customer: Option<String>,
}

async fn create_payment_intent(
    State(sim): State<StripeSimulator>,
    Json(payload): Json<CreatePaymentIntentPayload>,
) -> Result<(StatusCode, Json<StripePaymentIntent>), (StatusCode, Json<StripeErrorResponse>)> {
    if payload.amount <= 0 {
        return Err(stripe_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            Some("parameter_invalid_integer"),
            Some("amount"),
            "Invalid positive integer: amount must be greater than 0",
        ));
    }

    if payload.currency.trim().is_empty() {
        return Err(stripe_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            Some("parameter_missing"),
            Some("currency"),
            "Missing required param: currency",
        ));
    }

    let mut lock = sim.state.write().await;
    if let Some(ref cust_id) = payload.customer {
        if !lock.customers.contains_key(cust_id) {
            return Err(stripe_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                Some("resource_missing"),
                Some("customer"),
                format!("No such customer: '{}'", cust_id),
            ));
        }
    }

    let id = format!("pi_test_{}", Uuid::new_v4().simple());
    let secret = format!("{}_secret_{}", id, Uuid::new_v4().simple());

    let intent = StripePaymentIntent {
        id: id.clone(),
        object: "payment_intent".to_string(),
        amount: payload.amount,
        currency: payload.currency,
        customer: payload.customer,
        status: "succeeded".to_string(),
        client_secret: secret,
        created: Utc::now().timestamp(),
    };

    lock.payment_intents.insert(id.clone(), intent.clone());
    drop(lock);

    sim.event_bus
        .emit("stripe", "payment_intent.succeeded", serde_json::to_value(&intent).unwrap())
        .await;

    Ok((StatusCode::OK, Json(intent)))
}

async fn get_payment_intent(
    State(sim): State<StripeSimulator>,
    Path(id): Path<String>,
) -> Result<Json<StripePaymentIntent>, (StatusCode, Json<StripeErrorResponse>)> {
    let lock = sim.state.read().await;
    lock.payment_intents
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| {
            stripe_error(
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                Some("resource_missing"),
                Some("id"),
                format!("No such payment_intent: '{}'", id),
            )
        })
}

#[derive(Deserialize)]
pub struct CreateCheckoutSessionPayload {
    pub customer: Option<String>,
    pub amount_total: Option<i64>,
    pub currency: Option<String>,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
    pub mode: Option<String>,
}

async fn create_checkout_session(
    State(sim): State<StripeSimulator>,
    Json(payload): Json<CreateCheckoutSessionPayload>,
) -> Result<(StatusCode, Json<StripeCheckoutSession>), (StatusCode, Json<StripeErrorResponse>)> {
    if let Some(amount) = payload.amount_total {
        if amount <= 0 {
            return Err(stripe_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                Some("parameter_invalid_integer"),
                Some("amount_total"),
                "Invalid positive integer: amount_total must be greater than 0",
            ));
        }
    }

    if let Some(ref curr) = payload.currency {
        if curr.trim().is_empty() {
            return Err(stripe_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                Some("parameter_invalid_empty"),
                Some("currency"),
                "Currency must not be empty",
            ));
        }
    }

    let mut lock = sim.state.write().await;
    if let Some(ref cust_id) = payload.customer {
        if !lock.customers.contains_key(cust_id) {
            return Err(stripe_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                Some("resource_missing"),
                Some("customer"),
                format!("No such customer: '{}'", cust_id),
            ));
        }
    }

    let id = format!("cs_test_{}", Uuid::new_v4().simple());
    let checkout_url = format!("{}/simulate/checkout/{}", sim.base_url, id);

    let session = StripeCheckoutSession {
        id: id.clone(),
        object: "checkout.session".to_string(),
        customer: payload.customer,
        amount_total: Some(payload.amount_total.unwrap_or(4900)),
        currency: Some(payload.currency.unwrap_or_else(|| "usd".to_string())),
        status: "open".to_string(),
        payment_status: "unpaid".to_string(),
        url: checkout_url,
        success_url: payload.success_url,
        cancel_url: payload.cancel_url,
        created: Utc::now().timestamp(),
    };

    lock.checkout_sessions.insert(id.clone(), session.clone());
    drop(lock);

    sim.event_bus
        .emit("stripe", "checkout.session.created", serde_json::to_value(&session).unwrap())
        .await;

    Ok((StatusCode::OK, Json(session)))
}

async fn get_checkout_session(
    State(sim): State<StripeSimulator>,
    Path(id): Path<String>,
) -> Result<Json<StripeCheckoutSession>, (StatusCode, Json<StripeErrorResponse>)> {
    let lock = sim.state.read().await;
    lock.checkout_sessions
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| {
            stripe_error(
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                Some("resource_missing"),
                Some("id"),
                format!("No such checkout.session: '{}'", id),
            )
        })
}

async fn complete_checkout_session(
    State(sim): State<StripeSimulator>,
    Path(id): Path<String>,
) -> Result<Json<StripeCheckoutSession>, (StatusCode, Json<StripeErrorResponse>)> {
    let mut lock = sim.state.write().await;
    if let Some(session) = lock.checkout_sessions.get_mut(&id) {
        session.status = "complete".to_string();
        session.payment_status = "paid".to_string();
        let cloned = session.clone();
        drop(lock);

        sim.event_bus
            .emit("stripe", "checkout.session.completed", serde_json::to_value(&cloned).unwrap())
            .await;

        Ok(Json(cloned))
    } else {
        Err(stripe_error(
            StatusCode::NOT_FOUND,
            "invalid_request_error",
            Some("resource_missing"),
            Some("id"),
            format!("No such checkout.session: '{}'", id),
        ))
    }
}

#[derive(Deserialize)]
pub struct CreateSubscriptionPayload {
    pub customer: String,
}

async fn create_subscription(
    State(sim): State<StripeSimulator>,
    Json(payload): Json<CreateSubscriptionPayload>,
) -> Result<(StatusCode, Json<StripeSubscription>), (StatusCode, Json<StripeErrorResponse>)> {
    let mut lock = sim.state.write().await;
    if !lock.customers.contains_key(&payload.customer) {
        return Err(stripe_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            Some("resource_missing"),
            Some("customer"),
            format!("No such customer: '{}'", payload.customer),
        ));
    }

    let id = format!("sub_test_{}", Uuid::new_v4().simple());
    let sub = StripeSubscription {
        id: id.clone(),
        object: "subscription".to_string(),
        customer: payload.customer,
        status: "active".to_string(),
        current_period_end: Utc::now().timestamp() + 30 * 86400,
        created: Utc::now().timestamp(),
    };

    lock.subscriptions.insert(id.clone(), sub.clone());
    drop(lock);

    sim.event_bus
        .emit("stripe", "customer.subscription.created", serde_json::to_value(&sub).unwrap())
        .await;

    Ok((StatusCode::OK, Json(sub)))
}

async fn get_subscription(
    State(sim): State<StripeSimulator>,
    Path(id): Path<String>,
) -> Result<Json<StripeSubscription>, (StatusCode, Json<StripeErrorResponse>)> {
    let lock = sim.state.read().await;
    lock.subscriptions
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| {
            stripe_error(
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                Some("resource_missing"),
                Some("id"),
                format!("No such subscription: '{}'", id),
            )
        })
}

async fn cancel_subscription(
    State(sim): State<StripeSimulator>,
    Path(id): Path<String>,
) -> Result<Json<StripeSubscription>, (StatusCode, Json<StripeErrorResponse>)> {
    let mut lock = sim.state.write().await;
    if let Some(sub) = lock.subscriptions.get_mut(&id) {
        sub.status = "canceled".to_string();
        let cloned = sub.clone();
        drop(lock);

        sim.event_bus
            .emit("stripe", "customer.subscription.deleted", serde_json::to_value(&cloned).unwrap())
            .await;

        Ok(Json(cloned))
    } else {
        Err(stripe_error(
            StatusCode::NOT_FOUND,
            "invalid_request_error",
            Some("resource_missing"),
            Some("id"),
            format!("No such subscription: '{}'", id),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_stripe_simulator_lifecycle() {
        let bus = EventBus::new();
        let sim = StripeSimulator::new(bus.clone(), "http://localhost:8787".to_string());

        // 1. Create customer
        let cust_payload = CreateCustomerPayload {
            email: Some("alice@example.com".to_string()),
            name: Some("Alice".to_string()),
        };
        let (_, Json(cust)) = create_customer(State(sim.clone()), Json(cust_payload)).await;
        assert!(cust.id.starts_with("cus_test_"));

        // 2. Create Payment Intent
        let pi_payload = CreatePaymentIntentPayload {
            amount: 2500,
            currency: "usd".to_string(),
            customer: Some(cust.id.clone()),
        };
        let (_, Json(pi)) = create_payment_intent(State(sim.clone()), Json(pi_payload))
            .await
            .unwrap();
        assert_eq!(pi.status, "succeeded");

        // 3. Create Checkout Session with dynamic amount & currency, plus default fallback
        let cs_dynamic = CreateCheckoutSessionPayload {
            customer: Some(cust.id.clone()),
            amount_total: Some(9900),
            currency: Some("eur".to_string()),
            success_url: None,
            cancel_url: None,
            mode: None,
        };
        let (_, Json(session_dyn)) = create_checkout_session(State(sim.clone()), Json(cs_dynamic))
            .await
            .unwrap();
        assert_eq!(session_dyn.amount_total, Some(9900));
        assert_eq!(session_dyn.currency.as_deref(), Some("eur"));

        let cs_default = CreateCheckoutSessionPayload {
            customer: None,
            amount_total: None,
            currency: None,
            success_url: None,
            cancel_url: None,
            mode: None,
        };
        let (_, Json(session_def)) = create_checkout_session(State(sim.clone()), Json(cs_default))
            .await
            .unwrap();
        assert_eq!(session_def.amount_total, Some(4900));
        assert_eq!(session_def.currency.as_deref(), Some("usd"));

        // 4. Create Subscription & Cancel
        let sub_payload = CreateSubscriptionPayload {
            customer: cust.id.clone(),
        };
        let (_, Json(sub)) = create_subscription(State(sim.clone()), Json(sub_payload))
            .await
            .unwrap();
        assert_eq!(sub.status, "active");

        let Json(canceled) = cancel_subscription(State(sim.clone()), Path(sub.id)).await.unwrap();
        assert_eq!(canceled.status, "canceled");

        // 5. Verify EventBus captured all 6 events (including checkout.session.created x2)
        let events = bus.list(Some("stripe"), 10).await;
        assert_eq!(events.len(), 6);
        assert!(events.iter().any(|e| e.event_type == "checkout.session.created"));

        // 6. Reset clears all Stripe stores
        sim.reset().await;
        assert!(get_customer(State(sim.clone()), Path(cust.id)).await.is_err());
    }

    #[tokio::test]
    async fn test_stripe_validation_errors() {
        let bus = EventBus::new();
        let sim = StripeSimulator::new(bus, "http://localhost:8787".to_string());

        // Non-positive PaymentIntent amount -> 400 Stripe error JSON
        let bad_pi = CreatePaymentIntentPayload {
            amount: 0,
            currency: "usd".to_string(),
            customer: None,
        };
        let (status, Json(err)) = create_payment_intent(State(sim.clone()), Json(bad_pi))
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(err.error.error_type, "invalid_request_error");
        assert_eq!(err.error.param.as_deref(), Some("amount"));

        // Negative amount
        let neg_pi = CreatePaymentIntentPayload {
            amount: -100,
            currency: "usd".to_string(),
            customer: None,
        };
        let (status, _) = create_payment_intent(State(sim.clone()), Json(neg_pi))
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Missing customer on checkout session -> 400 resource_missing
        let missing_cust_cs = CreateCheckoutSessionPayload {
            customer: Some("cus_does_not_exist".to_string()),
            amount_total: Some(1500),
            currency: Some("usd".to_string()),
            success_url: None,
            cancel_url: None,
            mode: None,
        };
        let (status, Json(err)) = create_checkout_session(State(sim.clone()), Json(missing_cust_cs))
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(err.error.code.as_deref(), Some("resource_missing"));
        assert_eq!(err.error.param.as_deref(), Some("customer"));
    }
}

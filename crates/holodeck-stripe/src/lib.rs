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
) -> Result<Json<StripeCustomer>, StatusCode> {
    let lock = sim.state.read().await;
    lock.customers.get(&id).cloned().map(Json).ok_or(StatusCode::NOT_FOUND)
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
) -> (StatusCode, Json<StripePaymentIntent>) {
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

    let mut lock = sim.state.write().await;
    lock.payment_intents.insert(id.clone(), intent.clone());

    sim.event_bus
        .emit("stripe", "payment_intent.succeeded", serde_json::to_value(&intent).unwrap())
        .await;

    (StatusCode::OK, Json(intent))
}

async fn get_payment_intent(
    State(sim): State<StripeSimulator>,
    Path(id): Path<String>,
) -> Result<Json<StripePaymentIntent>, StatusCode> {
    let lock = sim.state.read().await;
    lock.payment_intents.get(&id).cloned().map(Json).ok_or(StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
pub struct CreateCheckoutSessionPayload {
    pub customer: Option<String>,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
    pub mode: Option<String>,
}

async fn create_checkout_session(
    State(sim): State<StripeSimulator>,
    Json(payload): Json<CreateCheckoutSessionPayload>,
) -> impl IntoResponse {
    let id = format!("cs_test_{}", Uuid::new_v4().simple());
    let checkout_url = format!("{}/simulate/checkout/{}", sim.base_url, id);

    let session = StripeCheckoutSession {
        id: id.clone(),
        object: "checkout.session".to_string(),
        customer: payload.customer,
        amount_total: Some(4900),
        currency: Some("usd".to_string()),
        status: "open".to_string(),
        payment_status: "unpaid".to_string(),
        url: checkout_url,
        success_url: payload.success_url,
        cancel_url: payload.cancel_url,
        created: Utc::now().timestamp(),
    };

    let mut lock = sim.state.write().await;
    lock.checkout_sessions.insert(id.clone(), session.clone());

    (StatusCode::OK, Json(session))
}

async fn get_checkout_session(
    State(sim): State<StripeSimulator>,
    Path(id): Path<String>,
) -> Result<Json<StripeCheckoutSession>, StatusCode> {
    let lock = sim.state.read().await;
    lock.checkout_sessions.get(&id).cloned().map(Json).ok_or(StatusCode::NOT_FOUND)
}

async fn complete_checkout_session(
    State(sim): State<StripeSimulator>,
    Path(id): Path<String>,
) -> Result<Json<StripeCheckoutSession>, StatusCode> {
    let mut lock = sim.state.write().await;
    if let Some(session) = lock.checkout_sessions.get_mut(&id) {
        session.status = "complete".to_string();
        session.payment_status = "paid".to_string();
        let cloned = session.clone();

        sim.event_bus
            .emit("stripe", "checkout.session.completed", serde_json::to_value(&cloned).unwrap())
            .await;

        Ok(Json(cloned))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

#[derive(Deserialize)]
pub struct CreateSubscriptionPayload {
    pub customer: String,
}

async fn create_subscription(
    State(sim): State<StripeSimulator>,
    Json(payload): Json<CreateSubscriptionPayload>,
) -> (StatusCode, Json<StripeSubscription>) {
    let id = format!("sub_test_{}", Uuid::new_v4().simple());
    let sub = StripeSubscription {
        id: id.clone(),
        object: "subscription".to_string(),
        customer: payload.customer,
        status: "active".to_string(),
        current_period_end: Utc::now().timestamp() + 30 * 86400,
        created: Utc::now().timestamp(),
    };

    let mut lock = sim.state.write().await;
    lock.subscriptions.insert(id.clone(), sub.clone());

    sim.event_bus
        .emit("stripe", "customer.subscription.created", serde_json::to_value(&sub).unwrap())
        .await;

    (StatusCode::OK, Json(sub))
}

async fn get_subscription(
    State(sim): State<StripeSimulator>,
    Path(id): Path<String>,
) -> Result<Json<StripeSubscription>, StatusCode> {
    let lock = sim.state.read().await;
    lock.subscriptions.get(&id).cloned().map(Json).ok_or(StatusCode::NOT_FOUND)
}

async fn cancel_subscription(
    State(sim): State<StripeSimulator>,
    Path(id): Path<String>,
) -> Result<Json<StripeSubscription>, StatusCode> {
    let mut lock = sim.state.write().await;
    if let Some(sub) = lock.subscriptions.get_mut(&id) {
        sub.status = "canceled".to_string();
        let cloned = sub.clone();

        sim.event_bus
            .emit("stripe", "customer.subscription.deleted", serde_json::to_value(&cloned).unwrap())
            .await;

        Ok(Json(cloned))
    } else {
        Err(StatusCode::NOT_FOUND)
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
        let (_, Json(pi)) = create_payment_intent(State(sim.clone()), Json(pi_payload)).await;
        assert_eq!(pi.status, "succeeded");

        // 3. Create Subscription & Cancel
        let sub_payload = CreateSubscriptionPayload {
            customer: cust.id.clone(),
        };
        let (_, Json(sub)) = create_subscription(State(sim.clone()), Json(sub_payload)).await;
        assert_eq!(sub.status, "active");

        let Json(canceled) = cancel_subscription(State(sim.clone()), Path(sub.id)).await.unwrap();
        assert_eq!(canceled.status, "canceled");

        // 4. Verify EventBus captured all 3 events
        let events = bus.list(Some("stripe"), 10).await;
        assert_eq!(events.len(), 4);
    }
}

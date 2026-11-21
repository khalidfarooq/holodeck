use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Form, Json, Router,
};
use chrono::Utc;
use holodeck_core::events::EventBus;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedEmail {
    pub id: String,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedSms {
    pub id: String,
    pub from: String,
    pub to: String,
    pub body: String,
    pub timestamp: i64,
}

#[derive(Default)]
pub struct NotificationState {
    pub emails: Vec<CapturedEmail>,
    pub sms: Vec<CapturedSms>,
}

#[derive(Clone)]
pub struct NotificationSimulator {
    pub state: Arc<RwLock<NotificationState>>,
    pub event_bus: EventBus,
}

impl NotificationSimulator {
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            state: Arc::new(RwLock::new(NotificationState::default())),
            event_bus,
        }
    }

    pub fn router(self) -> Router {
        Router::new()
            .route("/emails", post(resend_send_email))
            .route("/v1/emails", post(resend_send_email))
            .route("/v3/mail/send", post(sendgrid_send_mail))
            .route(
                "/2010-04-01/Accounts/:account_sid/Messages.json",
                post(twilio_send_sms),
            )
            .route("/inbox/emails", get(list_emails))
            .route("/inbox/sms", get(list_sms))
            .with_state(self)
    }
}

// Resend Payload
#[derive(Deserialize)]
pub struct ResendPayload {
    pub from: String,
    pub to: serde_json::Value, // string or array of strings
    pub subject: String,
    pub html: Option<String>,
    pub text: Option<String>,
}

async fn resend_send_email(
    State(sim): State<NotificationSimulator>,
    Json(payload): Json<ResendPayload>,
) -> impl IntoResponse {
    let to_vec: Vec<String> = if let Some(arr) = payload.to.as_array() {
        arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
    } else if let Some(s) = payload.to.as_str() {
        vec![s.to_string()]
    } else {
        vec!["recipient@example.com".to_string()]
    };

    let id = format!("re_{}", Uuid::new_v4().simple());
    let body = payload.html.or(payload.text).unwrap_or_default();

    let email = CapturedEmail {
        id: id.clone(),
        from: payload.from,
        to: to_vec.clone(),
        subject: payload.subject,
        body,
        timestamp: Utc::now().timestamp(),
    };

    let mut lock = sim.state.write().await;
    lock.emails.push(email.clone());

    sim.event_bus
        .emit("email", "email.sent", serde_json::to_value(&email).unwrap())
        .await;

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "id": id,
            "from": email.from,
            "to": to_vec,
        })),
    )
}

// SendGrid Payload
async fn sendgrid_send_mail(
    State(sim): State<NotificationSimulator>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let email = CapturedEmail {
        id: format!("sg_{}", Uuid::new_v4().simple()),
        from: payload["from"]["email"].as_str().unwrap_or("sendgrid@mock.com").to_string(),
        to: vec!["recipient@mock.com".to_string()],
        subject: payload["subject"].as_str().unwrap_or("No Subject").to_string(),
        body: payload.to_string(),
        timestamp: Utc::now().timestamp(),
    };

    let mut lock = sim.state.write().await;
    lock.emails.push(email.clone());

    sim.event_bus
        .emit("email", "email.sent", serde_json::to_value(&email).unwrap())
        .await;

    StatusCode::ACCEPTED
}

// Twilio SMS
#[derive(Deserialize)]
pub struct TwilioSmsForm {
    #[serde(rename = "From")]
    pub from: Option<String>,
    #[serde(rename = "To")]
    pub to: Option<String>,
    #[serde(rename = "Body")]
    pub body: Option<String>,
}

async fn twilio_send_sms(
    State(sim): State<NotificationSimulator>,
    Form(form): Form<TwilioSmsForm>,
) -> impl IntoResponse {
    let sid = format!("SM_{}", Uuid::new_v4().simple());
    let sms = CapturedSms {
        id: sid.clone(),
        from: form.from.unwrap_or_else(|| "+15551234567".to_string()),
        to: form.to.unwrap_or_else(|| "+15559876543".to_string()),
        body: form.body.unwrap_or_default(),
        timestamp: Utc::now().timestamp(),
    };

    let mut lock = sim.state.write().await;
    lock.sms.push(sms.clone());

    sim.event_bus
        .emit("sms", "sms.sent", serde_json::to_value(&sms).unwrap())
        .await;

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "sid": sid,
            "status": "queued",
            "from": sms.from,
            "to": sms.to,
            "body": sms.body
        })),
    )
}

async fn list_emails(State(sim): State<NotificationSimulator>) -> Json<Vec<CapturedEmail>> {
    let lock = sim.state.read().await;
    Json(lock.emails.clone())
}

async fn list_sms(State(sim): State<NotificationSimulator>) -> Json<Vec<CapturedSms>> {
    let lock = sim.state.read().await;
    Json(lock.sms.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_notifications_lifecycle() {
        let bus = EventBus::new();
        let sim = NotificationSimulator::new(bus.clone());

        // 1. Resend Email
        let resend_payload = ResendPayload {
            from: "auth@myapp.com".to_string(),
            to: serde_json::json!("user@example.com"),
            subject: "Your OTP Code".to_string(),
            html: Some("<p>Your code is 4242</p>".to_string()),
            text: None,
        };

        let res = resend_send_email(State(sim.clone()), Json(resend_payload)).await.into_response();
        assert_eq!(res.status(), StatusCode::OK);

        // 2. Twilio SMS
        let twilio_form = TwilioSmsForm {
            from: Some("+15550001".to_string()),
            to: Some("+15550002".to_string()),
            body: Some("Verification code: 9999".to_string()),
        };

        let sms_res = twilio_send_sms(State(sim.clone()), Form(twilio_form)).await.into_response();
        assert_eq!(sms_res.status(), StatusCode::CREATED);

        // 3. Inspect inbox
        let emails = list_emails(State(sim.clone())).await;
        assert_eq!(emails.len(), 1);
        assert_eq!(emails[0].subject, "Your OTP Code");

        let sms_list = list_sms(State(sim.clone())).await;
        assert_eq!(sms_list.len(), 1);
        assert_eq!(sms_list[0].body, "Verification code: 9999");

        // 4. Verify EventBus
        let all_events = bus.list(None, 10).await;
        assert_eq!(all_events.len(), 2);
    }
}

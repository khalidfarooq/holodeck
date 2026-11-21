use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockEvent {
    pub id: String,
    pub provider: String,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone)]
pub struct EventBus {
    events: Arc<RwLock<Vec<MockEvent>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            events: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Emits a new event into the event bus
    pub async fn emit(&self, provider: &str, event_type: &str, payload: serde_json::Value) -> MockEvent {
        let event = MockEvent {
            id: format!("evt_{}", Uuid::new_v4().simple()),
            provider: provider.to_string(),
            event_type: event_type.to_string(),
            payload,
            timestamp: Utc::now(),
        };

        let mut lock = self.events.write().await;
        lock.push(event.clone());
        event
    }

    /// Queries captured events by provider with optional limit
    pub async fn list(&self, provider: Option<&str>, limit: usize) -> Vec<MockEvent> {
        let lock = self.events.read().await;
        let mut filtered: Vec<MockEvent> = lock
            .iter()
            .filter(|e| match provider {
                Some(p) => e.provider.eq_ignore_ascii_case(p),
                None => true,
            })
            .cloned()
            .collect();

        filtered.reverse(); // newest first
        filtered.truncate(limit);
        filtered
    }

    /// Clears events, optionally filtered by provider
    pub async fn clear(&self, provider: Option<&str>) {
        let mut lock = self.events.write().await;
        if let Some(p) = provider {
            lock.retain(|e| !e.provider.eq_ignore_ascii_case(p));
        } else {
            lock.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_event_bus() {
        let bus = EventBus::new();
        bus.emit("stripe", "charge.succeeded", serde_json::json!({"amount": 2000})).await;
        bus.emit("email", "email.sent", serde_json::json!({"to": "user@example.com"})).await;

        let stripe_events = bus.list(Some("stripe"), 10).await;
        assert_eq!(stripe_events.len(), 1);
        assert_eq!(stripe_events[0].event_type, "charge.succeeded");

        let all_events = bus.list(None, 10).await;
        assert_eq!(all_events.len(), 2);

        bus.clear(Some("stripe")).await;
        let stripe_after = bus.list(Some("stripe"), 10).await;
        assert_eq!(stripe_after.len(), 0);
        let email_after = bus.list(Some("email"), 10).await;
        assert_eq!(email_after.len(), 1);
    }
}

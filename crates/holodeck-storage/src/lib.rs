use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{post, put},
    Json, Router,
};
use holodeck_core::events::EventBus;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Default)]
pub struct StorageState {
    pub objects: HashMap<String, (Bytes, String)>, // "bucket/key" -> (bytes, content_type)
}

#[derive(Clone)]
pub struct StorageSimulator {
    pub state: Arc<RwLock<StorageState>>,
    pub event_bus: EventBus,
    pub base_url: String,
}

impl StorageSimulator {
    pub fn new(event_bus: EventBus, base_url: String) -> Self {
        Self {
            state: Arc::new(RwLock::new(StorageState::default())),
            event_bus,
            base_url,
        }
    }

    pub async fn reset(&self) {
        let mut lock = self.state.write().await;
        lock.objects.clear();
    }

    pub fn router(self) -> Router {
        Router::new()
            .route("/storage/presign", post(generate_presigned_url))
            .route("/storage/:bucket/*key", put(upload_object).get(get_object).delete(delete_object))
            .with_state(self)
    }
}

#[derive(Deserialize)]
pub struct PresignRequest {
    pub bucket: String,
    pub key: String,
    pub method: Option<String>,
}

#[derive(Serialize)]
pub struct PresignResponse {
    pub url: String,
    pub bucket: String,
    pub key: String,
    pub method: String,
    pub expires_in_seconds: u32,
}

async fn generate_presigned_url(
    State(sim): State<StorageSimulator>,
    Json(req): Json<PresignRequest>,
) -> Json<PresignResponse> {
    let method = req.method.unwrap_or_else(|| "PUT".to_string()).to_uppercase();
    let url = format!("{}/storage/{}/{}", sim.base_url, req.bucket, req.key);

    sim.event_bus
        .emit(
            "storage",
            "storage.presigned_url_created",
            serde_json::json!({
                "bucket": req.bucket,
                "key": req.key,
                "url": url,
            }),
        )
        .await;

    Json(PresignResponse {
        url,
        bucket: req.bucket,
        key: req.key,
        method,
        expires_in_seconds: 900,
    })
}

async fn upload_object(
    State(sim): State<StorageSimulator>,
    Path((bucket, key)): Path<(String, String)>,
    body: Bytes,
) -> impl IntoResponse {
    let storage_key = format!("{}/{}", bucket, key);
    let size = body.len();

    let mut lock = sim.state.write().await;
    lock.objects.insert(storage_key, (body, "application/octet-stream".to_string()));

    sim.event_bus
        .emit(
            "storage",
            "storage.object_created",
            serde_json::json!({
                "bucket": bucket,
                "key": key,
                "size_bytes": size,
            }),
        )
        .await;

    (StatusCode::OK, Json(serde_json::json!({"status": "uploaded", "size": size})))
}

async fn get_object(
    State(sim): State<StorageSimulator>,
    Path((bucket, key)): Path<(String, String)>,
) -> impl IntoResponse {
    let storage_key = format!("{}/{}", bucket, key);
    let lock = sim.state.read().await;

    if let Some((bytes, content_type)) = lock.objects.get(&storage_key) {
        (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type.clone()),
                (header::CONTENT_LENGTH, bytes.len().to_string()),
            ],
            bytes.clone(),
        )
            .into_response()
    } else {
        (StatusCode::NOT_FOUND, "Object not found").into_response()
    }
}

async fn delete_object(
    State(sim): State<StorageSimulator>,
    Path((bucket, key)): Path<(String, String)>,
) -> impl IntoResponse {
    let storage_key = format!("{}/{}", bucket, key);
    let mut lock = sim.state.write().await;

    if lock.objects.remove(&storage_key).is_some() {
        sim.event_bus
            .emit(
                "storage",
                "storage.object_deleted",
                serde_json::json!({
                    "bucket": bucket,
                    "key": key,
                }),
            )
            .await;
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_storage_lifecycle() {
        let bus = EventBus::new();
        let sim = StorageSimulator::new(bus.clone(), "http://localhost:8787".to_string());

        // 1. Generate presigned URL
        let presign_req = PresignRequest {
            bucket: "avatars".to_string(),
            key: "user1.png".to_string(),
            method: Some("PUT".to_string()),
        };
        let Json(presign) = generate_presigned_url(State(sim.clone()), Json(presign_req)).await;
        assert!(presign.url.contains("/storage/avatars/user1.png"));

        // 2. Upload file bytes
        let file_bytes = Bytes::from_static(b"fake image data 123");
        let upload_res = upload_object(
            State(sim.clone()),
            Path(("avatars".to_string(), "user1.png".to_string())),
            file_bytes.clone(),
        )
        .await
        .into_response();
        assert_eq!(upload_res.status(), StatusCode::OK);

        // 3. Read file bytes
        let get_res = get_object(
            State(sim.clone()),
            Path(("avatars".to_string(), "user1.png".to_string())),
        )
        .await
        .into_response();
        assert_eq!(get_res.status(), StatusCode::OK);

        // 4. Verify EventBus
        let events = bus.list(Some("storage"), 10).await;
        assert_eq!(events.len(), 2);
    }
}

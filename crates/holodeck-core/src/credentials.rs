use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockCredentials {
    pub provider: String,
    pub api_key: String,
    pub webhook_secret: Option<String>,
    pub base_url: String,
    pub env_snippet: String,
}

pub fn get_mock_credentials(provider: &str) -> MockCredentials {
    match provider.to_lowercase().as_str() {
        "stripe" => MockCredentials {
            provider: "stripe".to_string(),
            api_key: "sk_test_holodeck_key_123".to_string(),
            webhook_secret: Some("whsec_test_holodeck_secret_key".to_string()),
            base_url: "http://127.0.0.1:8787".to_string(),
            env_snippet: r#"STRIPE_SECRET_KEY=sk_test_holodeck_key_123
STRIPE_WEBHOOK_SECRET=whsec_test_holodeck_secret_key
STRIPE_BASE_URL=http://127.0.0.1:8787"#
                .to_string(),
        },
        "oauth" | "google" | "github" => MockCredentials {
            provider: "oauth".to_string(),
            api_key: "holodeck_oauth_client_id".to_string(),
            webhook_secret: None,
            base_url: "http://127.0.0.1:8787/oauth".to_string(),
            env_snippet: r#"AUTH_CLIENT_ID=holodeck_oauth_client_id
AUTH_CLIENT_SECRET=holodeck_oauth_client_secret
AUTH_ISSUER=http://127.0.0.1:8787"#
                .to_string(),
        },
        "s3" | "aws" => MockCredentials {
            provider: "s3".to_string(),
            api_key: "holodeck_s3_key".to_string(),
            webhook_secret: None,
            base_url: "http://127.0.0.1:8787/storage".to_string(),
            env_snippet: r#"AWS_ACCESS_KEY_ID=holodeck_s3_key
AWS_SECRET_ACCESS_KEY=holodeck_s3_secret
AWS_ENDPOINT_URL_S3=http://127.0.0.1:8787/storage
AWS_REGION=us-east-1"#
                .to_string(),
        },
        "resend" | "email" => MockCredentials {
            provider: "resend".to_string(),
            api_key: "re_holodeck_test_key".to_string(),
            webhook_secret: None,
            base_url: "http://127.0.0.1:8787".to_string(),
            env_snippet: r#"RESEND_API_KEY=re_holodeck_test_key
RESEND_BASE_URL=http://127.0.0.1:8787"#
                .to_string(),
        },
        _ => MockCredentials {
            provider: provider.to_string(),
            api_key: format!("test_{}_key", provider),
            webhook_secret: Some(format!("whsec_{}_secret", provider)),
            base_url: "http://127.0.0.1:8787".to_string(),
            env_snippet: format!("HOLODECK_{}_API_KEY=test_key", provider.to_uppercase()),
        },
    }
}

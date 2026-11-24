use clap::{Parser, Subcommand};
use holodeck_core::credentials::get_mock_credentials;
use holodeck_core::dispatcher::WebhookDispatcher;
use holodeck_core::events::EventBus;
use holodeck_mcp::HolodeckMcpServer;
use holodeck_server::HolodeckApp;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(
    name = "holodeck",
    author = "Khalid Farooq <wandererkhalid@gmail.com>",
    version = "0.1.0",
    about = "🌌 Universal Local Mock-Cloud, SaaS & Webhook Simulator for AI Coding Agents",
    long_about = "Holodeck spins up a zero-config, in-memory local mock cloud (Stripe, OAuth2, S3, Resend, Twilio) and HMAC webhook injector with Model Context Protocol (MCP) support for autonomous AI coding agents."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the unified HTTP mock-cloud server
    Start {
        /// Port to bind the server to
        #[arg(short, long, default_value_t = 8787)]
        port: u16,

        /// Suppress welcome banner
        #[arg(long)]
        quiet: bool,
    },

    /// Start the Model Context Protocol (MCP) stdio server for AI agents
    Mcp {
        /// Base URL of a running Holodeck HTTP daemon (defaults to HOLODECK_URL or http://127.0.0.1:8787)
        #[arg(long, conflicts_with = "port")]
        url: Option<String>,

        /// Port of a running Holodeck HTTP daemon on 127.0.0.1
        #[arg(short, long, conflicts_with = "url")]
        port: Option<u16>,

        /// Run in standalone in-memory mode without connecting to an HTTP daemon
        #[arg(long, conflicts_with_all = ["url", "port"])]
        standalone: bool,
    },

    /// Dispatch a signed webhook payload to an endpoint
    Trigger {
        /// Provider signature standard ('stripe' or 'github')
        #[arg(short, long, default_value = "stripe")]
        provider: String,

        /// Webhook receiver URL (e.g. http://localhost:3000/api/webhooks/stripe)
        #[arg(short, long)]
        target: String,

        /// Event type name (e.g. checkout.session.completed)
        #[arg(short, long, default_value = "checkout.session.completed")]
        event: String,

        /// Signing secret
        #[arg(short, long, default_value = "whsec_test_holodeck_secret_key")]
        secret: String,

        /// Custom JSON payload string
        #[arg(long)]
        payload: Option<String>,
    },

    /// Check health and status of a running Holodeck server
    Status {
        /// Port of the running server
        #[arg(short, long, default_value_t = 8787)]
        port: u16,
    },

    /// Display mock environment variables for configuring AI agent test environments
    Env {
        /// Provider name ('stripe', 'oauth', 's3', 'resend', or 'all')
        #[arg(short, long, default_value = "all")]
        provider: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { port, quiet } => {
            // Initialize logging
            tracing_subscriber::registry()
                .with(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "info".into()),
                )
                .with(tracing_subscriber::fmt::layer())
                .init();

            if !quiet {
                print_banner(port);
            }

            let app = HolodeckApp::new(port);
            app.run(port).await?;
        }

        Commands::Mcp {
            url,
            port,
            standalone,
        } => {
            // MCP must output ONLY JSON-RPC over stdout, so logs go to stderr
            tracing_subscriber::registry()
                .with(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "warn".into()),
                )
                .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
                .init();

            let env_url = std::env::var("HOLODECK_URL").ok();
            let server = match resolve_mcp_daemon_url(url, port, standalone, env_url) {
                Some(base_url) => {
                    tracing::info!("Connecting Holodeck MCP server to daemon at {}", base_url);
                    Arc::new(HolodeckMcpServer::connect(base_url))
                }
                None => {
                    tracing::info!("Starting Holodeck MCP server in standalone in-memory mode");
                    Arc::new(HolodeckMcpServer::new(EventBus::new()))
                }
            };
            server.run_stdio().await?;
        }

        Commands::Trigger {
            provider,
            target,
            event,
            secret,
            payload,
        } => {
            let bus = EventBus::new();
            let dispatcher = WebhookDispatcher::new(bus);

            let parsed_payload: serde_json::Value = if let Some(p) = payload {
                serde_json::from_str(&p).expect("Invalid JSON payload provided")
            } else {
                serde_json::json!({
                    "id": format!("evt_test_{}", uuid::Uuid::new_v4().simple()),
                    "object": "event",
                    "type": event,
                    "created": chrono::Utc::now().timestamp(),
                    "data": {
                        "object": {
                            "id": format!("obj_test_{}", uuid::Uuid::new_v4().simple()),
                            "status": "completed",
                            "source": "holodeck-cli"
                        }
                    }
                })
            };

            println!("⚡ Dispatching HMAC signed webhook...");
            println!("   Provider: {}", provider);
            println!("   Target:   {}", target);
            println!("   Event:    {}", event);

            match dispatcher
                .dispatch(&provider, &target, &event, &parsed_payload, &secret)
                .await
            {
                Ok(result) => {
                    if result.success {
                        println!("✅ Webhook delivered successfully!");
                    } else {
                        println!("⚠️ Webhook delivered with error status.");
                    }
                    if let Some(sc) = result.status_code {
                        println!("   HTTP Status: {}", sc);
                    }
                    if !result.response_body.is_empty() {
                        println!("   Response:    {}", result.response_body);
                    }
                    if let Some(ref err) = result.error {
                        println!("   Error:       {}", err);
                    }
                }
                Err(e) => {
                    eprintln!("❌ Webhook delivery failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Status { port } => {
            let url = format!("http://127.0.0.1:{}/health", port);
            match reqwest::get(&url).await {
                Ok(res) if res.status().is_success() => {
                    let body: serde_json::Value = res.json().await.unwrap_or_default();
                    println!("✅ Holodeck server is running healthy at http://127.0.0.1:{}", port);
                    println!("Status Details:");
                    println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
                }
                Ok(res) => {
                    eprintln!("⚠️ Holodeck server responded with HTTP status: {}", res.status());
                }
                Err(err) => {
                    eprintln!("❌ Holodeck server is not running on port {}: {}", port, err);
                    std::process::exit(1);
                }
            }
        }

        Commands::Env { provider } => {
            if provider == "all" {
                for p in ["stripe", "oauth", "s3", "resend"] {
                    let creds = get_mock_credentials(p);
                    println!("# --- {} Mock Configuration ---", creds.provider.to_uppercase());
                    println!("{}\n", creds.env_snippet);
                }
            } else {
                let creds = get_mock_credentials(&provider);
                println!("# --- {} Mock Configuration ---", creds.provider.to_uppercase());
                println!("{}", creds.env_snippet);
            }
        }
    }

    Ok(())
}

fn resolve_mcp_daemon_url(
    url: Option<String>,
    port: Option<u16>,
    standalone: bool,
    env_url: Option<String>,
) -> Option<String> {
    if standalone {
        return None;
    }
    if let Some(u) = url {
        return Some(u);
    }
    if let Some(p) = port {
        return Some(format!("http://127.0.0.1:{}", p));
    }
    if let Some(u) = env_url.filter(|v| !v.trim().is_empty()) {
        return Some(u);
    }
    Some("http://127.0.0.1:8787".to_string())
}

fn print_banner(port: u16) {
    println!(
        r#"
  _  _     _         _         _   
 | || |___| |___  __| |___  __| |__
 | __ / _ \ / _ \/ _` / -_)/ _| / /
 |_||_\___/_\___/\__,_\___|\__|_\_\
 Universal Mock-Cloud for AI Coding Agents v0.1.0

🌐 Server running at: http://127.0.0.1:{port}

Available Mock Cloud Services:
  💳 Stripe API:         http://127.0.0.1:{port}/v1/checkout/sessions
                         http://127.0.0.1:{port}/v1/customers
                         http://127.0.0.1:{port}/v1/payment_intents
  🔑 OAuth2 / OIDC:      http://127.0.0.1:{port}/oauth/authorize
                         http://127.0.0.1:{port}/oauth/token
                         http://127.0.0.1:{port}/userinfo
  📦 S3 Storage:         http://127.0.0.1:{port}/storage/presign
                         http://127.0.0.1:{port}/storage/:bucket/:key
  📬 Notifications:      http://127.0.0.1:{port}/v1/emails (Resend)
                         http://127.0.0.1:{port}/v3/mail/send (SendGrid)
                         http://127.0.0.1:{port}/2010-04-01/Accounts/:sid/Messages.json (Twilio)
  🔍 Event Inspector:    http://127.0.0.1:{port}/events
  🔄 State Reset:        http://127.0.0.1:{port}/reset
  ⚡ Webhook Trigger:    http://127.0.0.1:{port}/webhook/trigger
  ❤️ Health Check:       http://127.0.0.1:{port}/health

Press Ctrl+C to stop.
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_mcp_daemon_url_precedence() {
        assert_eq!(
            resolve_mcp_daemon_url(
                Some("http://127.0.0.1:9000".into()),
                None,
                false,
                Some("http://127.0.0.1:9999".into())
            ),
            Some("http://127.0.0.1:9000".into())
        );
        assert_eq!(
            resolve_mcp_daemon_url(None, Some(9123), false, Some("http://127.0.0.1:9999".into())),
            Some("http://127.0.0.1:9123".into())
        );
        assert_eq!(
            resolve_mcp_daemon_url(None, None, false, Some("http://127.0.0.1:9999".into())),
            Some("http://127.0.0.1:9999".into())
        );
        assert_eq!(
            resolve_mcp_daemon_url(None, None, false, Some("   ".into())),
            Some("http://127.0.0.1:8787".into())
        );
        assert_eq!(
            resolve_mcp_daemon_url(None, None, false, None),
            Some("http://127.0.0.1:8787".into())
        );
        assert_eq!(
            resolve_mcp_daemon_url(Some("http://127.0.0.1:9000".into()), None, true, None),
            None
        );
    }
}

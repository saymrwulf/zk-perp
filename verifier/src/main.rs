//! Verifier binary for zk-perp
//!
//! A standalone verification node that:
//! - Reads batches and proofs from the DA layer
//! - Verifies ZK proofs independently
//! - Exposes an HTTP API for verification status
//!
//! ## Usage
//!
//! ```bash
//! # Run with default settings (mock verification, ./data directory)
//! cargo run --bin verifier
//!
//! # Run with custom settings
//! cargo run --bin verifier -- --data-dir /path/to/data --port 8081
//!
//! # Run with real ZK verification (requires risc0 feature)
//! cargo run --bin verifier --features risc0 -- --real-verification
//! ```
//!
//! ## API Endpoints
//!
//! - GET /health - Health check
//! - GET /stats - Verification statistics
//! - GET /batch/:id - Get batch verification status
//! - POST /verify - Verify next pending batch
//! - POST /verify/all - Verify all pending batches

use std::sync::Arc;
use axum::{
    routing::{get, post},
    Router, Json,
    extract::{State, Path},
    http::StatusCode,
};
use clap::Parser;
use serde::Serialize;
use tokio::sync::RwLock;
use tower_http::cors::{CorsLayer, Any};
use tracing::{info, error, Level};
use tracing_subscriber::FmtSubscriber;

use zk_perp_verifier::{
    Verifier, VerifierConfig, VerifierStats, BatchVerification, VerifierError,
};

/// CLI arguments
#[derive(Parser, Debug)]
#[command(name = "verifier")]
#[command(about = "ZK-Perp Verifier Node")]
#[command(version)]
struct Args {
    /// Data directory (where DA stores batches and proofs)
    #[arg(short, long, default_value = "./data")]
    data_dir: String,

    /// HTTP port
    #[arg(short, long, default_value_t = 8081)]
    port: u16,

    /// Use real ZK verification (default is mock)
    #[arg(long)]
    real_verification: bool,

    /// Automatically verify new batches on startup
    #[arg(long)]
    auto_verify: bool,
}

// ============================================================================
// Response Types
// ============================================================================

#[derive(Serialize)]
struct ApiResponse<T: Serialize> {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    fn err(error: impl ToString) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error.to_string()),
        }
    }
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
    verified_batches: u64,
    last_verified_batch: u64,
    use_mock: bool,
}

#[derive(Serialize)]
struct VerifyResponse {
    batches_verified: usize,
    results: Vec<BatchVerification>,
}

// ============================================================================
// Application State
// ============================================================================

struct AppState {
    verifier: RwLock<Verifier>,
}

// ============================================================================
// Endpoint Handlers
// ============================================================================

async fn health(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<HealthResponse>> {
    let verifier = state.verifier.read().await;
    let stats = verifier.stats();

    Json(ApiResponse::ok(HealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        verified_batches: stats.verified_batches,
        last_verified_batch: stats.last_verified_batch,
        use_mock: stats.use_mock_verifier,
    }))
}

async fn get_stats(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<VerifierStats>> {
    let verifier = state.verifier.read().await;
    Json(ApiResponse::ok(verifier.stats()))
}

async fn get_batch_status(
    State(state): State<Arc<AppState>>,
    Path(batch_id): Path<u64>,
) -> (StatusCode, Json<ApiResponse<BatchVerification>>) {
    let mut verifier = state.verifier.write().await;

    match verifier.get_batch_status(batch_id) {
        Ok(status) => (StatusCode::OK, Json(ApiResponse::ok(status))),
        Err(VerifierError::DaError(_)) => {
            (StatusCode::NOT_FOUND, Json(ApiResponse::err("Batch not found")))
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::err(e)))
        }
    }
}

async fn verify_next(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<ApiResponse<VerifyResponse>>) {
    let mut verifier = state.verifier.write().await;

    match verifier.verify_next() {
        Ok(Some(result)) => {
            info!("Verified batch {}", result.batch_id);
            (StatusCode::OK, Json(ApiResponse::ok(VerifyResponse {
                batches_verified: 1,
                results: vec![result],
            })))
        }
        Ok(None) => {
            (StatusCode::OK, Json(ApiResponse::ok(VerifyResponse {
                batches_verified: 0,
                results: vec![],
            })))
        }
        Err(e) => {
            error!("Verification failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::err(e)))
        }
    }
}

async fn verify_all(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<ApiResponse<VerifyResponse>>) {
    let mut verifier = state.verifier.write().await;

    match verifier.verify_all_pending() {
        Ok(results) => {
            let count = results.len();
            if count > 0 {
                info!("Verified {} batches", count);
            }
            (StatusCode::OK, Json(ApiResponse::ok(VerifyResponse {
                batches_verified: count,
                results,
            })))
        }
        Err(e) => {
            error!("Verification failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::err(e)))
        }
    }
}

/// Check if a specific batch is verified
async fn is_verified(
    State(state): State<Arc<AppState>>,
    Path(batch_id): Path<u64>,
) -> Json<ApiResponse<bool>> {
    let verifier = state.verifier.read().await;
    Json(ApiResponse::ok(verifier.is_verified(batch_id)))
}

// ============================================================================
// Main Entry Point
// ============================================================================

#[tokio::main]
async fn main() {
    // Parse CLI args
    let args = Args::parse();

    // Initialize logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set tracing subscriber");

    info!("Starting ZK-Perp Verifier v{}", env!("CARGO_PKG_VERSION"));
    info!("Data directory: {}", args.data_dir);
    info!("Verification mode: {}", if args.real_verification { "REAL ZK" } else { "MOCK" });

    // Create verifier
    let config = VerifierConfig {
        data_dir: args.data_dir.clone(),
        use_mock_verifier: !args.real_verification,
        port: args.port,
    };
    let mut verifier = Verifier::new(config);

    // Connect to DA layer
    match verifier.connect_da(&args.data_dir) {
        Ok(()) => info!("Connected to DA layer at {}", args.data_dir),
        Err(e) => {
            error!("Failed to connect to DA layer: {}", e);
            info!("Creating new DA directory at {}", args.data_dir);
            // The DA layer will create the directory if it doesn't exist
        }
    }

    // Auto-verify on startup if requested
    if args.auto_verify {
        info!("Auto-verifying pending batches...");
        match verifier.verify_all_pending() {
            Ok(results) => {
                if results.is_empty() {
                    info!("No pending batches to verify");
                } else {
                    info!("Verified {} batches on startup", results.len());
                }
            }
            Err(e) => error!("Auto-verification failed: {}", e),
        }
    }

    // Create application state
    let state = Arc::new(AppState {
        verifier: RwLock::new(verifier),
    });

    // Build router
    let app = Router::new()
        .route("/health", get(health))
        .route("/stats", get(get_stats))
        .route("/batch/:batch_id", get(get_batch_status))
        .route("/batch/:batch_id/verified", get(is_verified))
        .route("/verify", post(verify_next))
        .route("/verify/all", post(verify_all))
        .layer(CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any))
        .with_state(state);

    // Start server
    let addr = format!("0.0.0.0:{}", args.port);
    info!("Verifier API listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await
        .expect("Failed to bind to address");

    axum::serve(listener, app).await
        .expect("Server failed");
}

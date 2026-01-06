//! HTTP API for the sequencer
//!
//! Provides RESTful endpoints for:
//! - Account management (registration, balance queries)
//! - Transaction submission
//! - Market data (orderbook, prices)
//! - System status and statistics

use axum::{
    routing::{get, post},
    Router, Json,
    extract::{State, Path, Query},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::{Sequencer, SequencerError, SequencerStats};
use zk_perp_core::{
    transactions::*,
    types::*,
};
use zk_perp_da::DaStats;
use zk_perp_oracle::PriceSource;

// ============================================================================
// Response Types
// ============================================================================

/// Generic API response wrapper
#[derive(Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn err(error: impl ToString) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error.to_string()),
        }
    }
}

// ============================================================================
// Request/Response DTOs
// ============================================================================

/// Health check response
#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub block_number: u64,
    pub pending_transactions: usize,
}

/// System status response
#[derive(Serialize)]
pub struct StatusResponse {
    pub sequencer_stats: SequencerStats,
    pub da_stats: DaStats,
    pub markets: Vec<MarketInfo>,
}

#[derive(Serialize)]
pub struct MarketInfo {
    pub id: MarketId,
    pub name: String,
    pub base_asset: String,
    pub quote_asset: String,
    pub price: Price,
}

/// Register account request
#[derive(Deserialize)]
pub struct RegisterAccountRequest {
    pub public_key: String, // Hex-encoded public key
}

/// Register account response
#[derive(Serialize)]
pub struct RegisterAccountResponse {
    pub account_id: AccountId,
    pub public_key: String,
}

/// Account info response
#[derive(Serialize)]
pub struct AccountInfoResponse {
    pub account_id: AccountId,
    pub owner: String, // Hex-encoded
    pub nonce: u64,
    pub balances: Vec<BalanceInfo>,
}

#[derive(Serialize)]
pub struct BalanceInfo {
    pub asset_id: AssetId,
    pub asset_name: String,
    pub free: String,    // Formatted amount
    pub locked: String,  // Formatted amount
}

/// Submit transaction request
#[derive(Deserialize)]
pub struct SubmitTxRequest {
    pub transaction: SignedTransaction,
}

/// Transaction receipt response
#[derive(Serialize)]
pub struct TxReceiptResponse {
    pub tx_hash: String,
    pub success: bool,
    pub error: Option<String>,
    pub fills: Vec<FillInfo>,
    pub order_id: Option<OrderId>,
}

#[derive(Serialize)]
pub struct FillInfo {
    pub price: String,
    pub quantity: String,
    pub side: String,
    pub maker_order_id: OrderId,
}

/// Order book query params
#[derive(Deserialize)]
pub struct OrderBookQuery {
    #[serde(default = "default_depth")]
    pub depth: usize,
}

fn default_depth() -> usize { 20 }

/// Orderbook response
#[derive(Serialize)]
pub struct OrderBookResponse {
    pub market_id: MarketId,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    pub timestamp: Timestamp,
}

#[derive(Serialize)]
pub struct PriceLevel {
    pub price: String,
    pub quantity: String,
}

/// Positions response
#[derive(Serialize)]
pub struct PositionsResponse {
    pub positions: Vec<PositionInfo>,
}

#[derive(Serialize)]
pub struct PositionInfo {
    pub market_id: MarketId,
    pub market_name: String,
    pub side: String,
    pub size: String,
    pub entry_price: String,
    pub unrealized_pnl: String,
}

/// Batch trigger request
#[derive(Deserialize)]
pub struct TriggerBatchRequest {
    pub force: bool,
}

/// Batch result response
#[derive(Serialize)]
pub struct BatchResultResponse {
    pub batch_id: u64,
    pub tx_count: usize,
    pub pre_root: String,
    pub post_root: String,
    pub proof_generated: bool,
}

// ============================================================================
// Router Setup
// ============================================================================

/// Create the API router with all endpoints
pub fn create_router(sequencer: Arc<Sequencer>) -> Router {
    Router::new()
        // Health & Status
        .route("/health", get(health))
        .route("/status", get(status))

        // Account Management
        .route("/accounts", post(register_account))
        .route("/accounts/:account_id", get(get_account))
        .route("/accounts/:account_id/positions", get(get_positions))

        // Transactions
        .route("/tx", post(submit_transaction))

        // Market Data
        .route("/markets/:market_id/orderbook", get(get_orderbook))
        .route("/markets/:market_id/price", get(get_price))

        // Admin / Debug
        .route("/admin/batch", post(trigger_batch))

        .with_state(sequencer)
}

// ============================================================================
// Endpoint Handlers
// ============================================================================

/// Health check endpoint
async fn health(
    State(sequencer): State<Arc<Sequencer>>,
) -> Json<ApiResponse<HealthResponse>> {
    let state = sequencer.state.read().await;
    let pending = sequencer.pending_txs.read().await.len();

    Json(ApiResponse::ok(HealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        block_number: state.system.block_number,
        pending_transactions: pending,
    }))
}

/// System status endpoint
async fn status(
    State(sequencer): State<Arc<Sequencer>>,
) -> Json<ApiResponse<StatusResponse>> {
    let sequencer_stats = sequencer.get_stats().await;
    let da_stats = sequencer.get_da_stats().await;

    // Get market info with prices
    let oracle = sequencer.oracle.read().await;
    let markets = vec![
        MarketInfo {
            id: 0,
            name: "BTC-USDC".to_string(),
            base_asset: "BTC".to_string(),
            quote_asset: "USDC".to_string(),
            price: oracle.get_price(0).map(|p| p.price).unwrap_or(0),
        },
        MarketInfo {
            id: 1,
            name: "ETH-USDC".to_string(),
            base_asset: "ETH".to_string(),
            quote_asset: "USDC".to_string(),
            price: oracle.get_price(1).map(|p| p.price).unwrap_or(0),
        },
    ];

    Json(ApiResponse::ok(StatusResponse {
        sequencer_stats,
        da_stats,
        markets,
    }))
}

/// Register a new account
async fn register_account(
    State(sequencer): State<Arc<Sequencer>>,
    Json(req): Json<RegisterAccountRequest>,
) -> (StatusCode, Json<ApiResponse<RegisterAccountResponse>>) {
    // Parse public key from hex
    let public_key: PublicKey = match parse_hex_32(&req.public_key) {
        Ok(pk) => pk,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(ApiResponse::err(e))),
    };

    let account_id = sequencer.register_account(public_key).await;

    (StatusCode::CREATED, Json(ApiResponse::ok(RegisterAccountResponse {
        account_id,
        public_key: req.public_key,
    })))
}

/// Get account information
async fn get_account(
    State(sequencer): State<Arc<Sequencer>>,
    Path(account_id): Path<AccountId>,
) -> (StatusCode, Json<ApiResponse<AccountInfoResponse>>) {
    match sequencer.get_account(account_id).await {
        Some(account) => {
            let balances = account.balances.iter().map(|b| BalanceInfo {
                asset_id: b.asset_id,
                asset_name: asset_name(b.asset_id),
                free: format_amount(b.free, 18),
                locked: format_amount(b.locked, 18),
            }).collect();

            (StatusCode::OK, Json(ApiResponse::ok(AccountInfoResponse {
                account_id: account.id,
                owner: to_hex(&account.owner),
                nonce: account.nonce,
                balances,
            })))
        }
        None => (StatusCode::NOT_FOUND, Json(ApiResponse::err("Account not found"))),
    }
}

/// Get positions for an account
async fn get_positions(
    State(sequencer): State<Arc<Sequencer>>,
    Path(account_id): Path<AccountId>,
) -> Json<ApiResponse<PositionsResponse>> {
    let positions = sequencer.get_positions(account_id).await;

    let position_infos: Vec<PositionInfo> = positions.iter().map(|p| {
        PositionInfo {
            market_id: p.market_id,
            market_name: market_name(p.market_id),
            side: format!("{:?}", p.side),
            size: format_amount(p.size, 18),
            entry_price: format_price(p.entry_price),
            unrealized_pnl: "0.00".to_string(), // Would calculate with oracle price
        }
    }).collect();

    Json(ApiResponse::ok(PositionsResponse { positions: position_infos }))
}

/// Submit a signed transaction
async fn submit_transaction(
    State(sequencer): State<Arc<Sequencer>>,
    Json(req): Json<SubmitTxRequest>,
) -> (StatusCode, Json<ApiResponse<TxReceiptResponse>>) {
    match sequencer.submit_transaction(req.transaction).await {
        Ok(receipt) => {
            let fills: Vec<FillInfo> = receipt.fills.iter().map(|f| FillInfo {
                price: format_price(f.price),
                quantity: format_amount(f.quantity, 18),
                side: format!("{:?}", f.taker_side),
                maker_order_id: f.maker_order_id,
            }).collect();

            (StatusCode::OK, Json(ApiResponse::ok(TxReceiptResponse {
                tx_hash: to_hex(&receipt.tx_hash),
                success: receipt.success,
                error: receipt.error,
                fills,
                order_id: receipt.order_id,
            })))
        }
        Err(e) => {
            let status = match &e {
                SequencerError::AccountNotFound(_) => StatusCode::NOT_FOUND,
                SequencerError::InvalidSignature(_) => StatusCode::UNAUTHORIZED,
                SequencerError::InsufficientBalance => StatusCode::BAD_REQUEST,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, Json(ApiResponse::err(e)))
        }
    }
}

/// Get orderbook for a market
async fn get_orderbook(
    State(sequencer): State<Arc<Sequencer>>,
    Path(market_id): Path<MarketId>,
    Query(query): Query<OrderBookQuery>,
) -> Json<ApiResponse<OrderBookResponse>> {
    let snapshot = sequencer.get_orderbook(market_id, query.depth).await;

    let bids: Vec<PriceLevel> = snapshot.bids.iter().map(|(p, q)| PriceLevel {
        price: format_price(*p),
        quantity: format_amount(*q, 18),
    }).collect();

    let asks: Vec<PriceLevel> = snapshot.asks.iter().map(|(p, q)| PriceLevel {
        price: format_price(*p),
        quantity: format_amount(*q, 18),
    }).collect();

    Json(ApiResponse::ok(OrderBookResponse {
        market_id: snapshot.market_id,
        bids,
        asks,
        timestamp: snapshot.timestamp,
    }))
}

/// Get current price for a market
async fn get_price(
    State(sequencer): State<Arc<Sequencer>>,
    Path(market_id): Path<MarketId>,
) -> Json<ApiResponse<MarketInfo>> {
    let oracle = sequencer.oracle.read().await;

    match oracle.get_price(market_id) {
        Ok(price) => {
            Json(ApiResponse::ok(MarketInfo {
                id: market_id,
                name: market_name(market_id),
                base_asset: base_asset(market_id),
                quote_asset: "USDC".to_string(),
                price: price.price,
            }))
        }
        Err(_) => Json(ApiResponse::err("Market not found")),
    }
}

/// Trigger batch processing (admin endpoint)
async fn trigger_batch(
    State(sequencer): State<Arc<Sequencer>>,
    Json(_req): Json<TriggerBatchRequest>,
) -> (StatusCode, Json<ApiResponse<BatchResultResponse>>) {
    match sequencer.process_batch().await {
        Ok(Some(result)) => {
            (StatusCode::OK, Json(ApiResponse::ok(BatchResultResponse {
                batch_id: result.batch_id,
                tx_count: result.tx_count,
                pre_root: to_hex(&result.pre_root),
                post_root: to_hex(&result.post_root),
                proof_generated: result.proof_generated,
            })))
        }
        Ok(None) => {
            (StatusCode::OK, Json(ApiResponse::err("No pending transactions")))
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::err(e)))
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn parse_hex_32(hex: &str) -> Result<[u8; 32], String> {
    let hex = hex.trim_start_matches("0x");
    if hex.len() != 64 {
        return Err(format!("Expected 64 hex characters, got {}", hex.len()));
    }

    let bytes: Result<Vec<u8>, _> = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i+2], 16))
        .collect();

    let bytes = bytes.map_err(|e| format!("Invalid hex: {}", e))?;

    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

fn format_amount(amount: u128, decimals: u32) -> String {
    let divisor = 10u128.pow(decimals);
    let whole = amount / divisor;
    let frac = amount % divisor;
    format!("{}.{:0width$}", whole, frac, width = decimals as usize)
}

fn format_price(price: Price) -> String {
    // Price has 8 decimals
    let whole = price / 100_000_000;
    let frac = price % 100_000_000;
    format!("{}.{:08}", whole, frac)
}

fn asset_name(asset_id: AssetId) -> String {
    match asset_id {
        0 => "USDC".to_string(),
        1 => "BTC".to_string(),
        2 => "ETH".to_string(),
        _ => format!("ASSET_{}", asset_id),
    }
}

fn market_name(market_id: MarketId) -> String {
    match market_id {
        0 => "BTC-USDC".to_string(),
        1 => "ETH-USDC".to_string(),
        _ => format!("MARKET_{}", market_id),
    }
}

fn base_asset(market_id: MarketId) -> String {
    match market_id {
        0 => "BTC".to_string(),
        1 => "ETH".to_string(),
        _ => format!("BASE_{}", market_id),
    }
}

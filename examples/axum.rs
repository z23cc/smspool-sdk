//! Minimal Axum integration owned entirely by the consuming application.
//!
//! Starting this example does not call SMSPool. Only an explicit request to `/balance` performs a
//! read-only provider request.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;
use smspool::Client;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("SMSPOOL_API_KEY")?;
    let client = Client::builder(api_key).build()?;

    let app = Router::new()
        .route("/balance", get(balance))
        .with_state(client);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn balance(State(client): State<Client>) -> Result<Json<serde_json::Value>, AppError> {
    let balance = client.catalog().balance().await?;
    Ok(Json(json!({ "balance": balance.balance.to_string() })))
}

struct AppError(smspool::Error);

impl From<smspool::Error> for AppError {
    fn from(error: smspool::Error) -> Self {
        Self(error)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let _category_only = &self.0;
        (
            StatusCode::BAD_GATEWAY,
            "upstream SMS provider request failed",
        )
            .into_response()
    }
}

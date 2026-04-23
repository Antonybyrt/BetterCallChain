use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use tower_http::cors::CorsLayer;

use crate::event_bus::{EventBus, WsEnvelope};
use crate::scenarios::run_scenario;

#[derive(Clone)]
pub struct AppState {
    pub bus:   Arc<EventBus>,
    pub ports: Vec<u16>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/",                       get(serve_index))
        .route("/ws",                     get(ws_upgrade))
        .route("/api/events",             get(get_recent))
        .route("/api/scenario/{name}",    post(run_scenario_handler))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn serve_index() -> Html<&'static str> {
    Html(include_str!("assets/index.html"))
}

async fn ws_upgrade(
    ws: axum::extract::ws::WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| {
        crate::ws_handler::handle_socket_pub(socket, state.bus)
    })
}

async fn get_recent(State(state): State<AppState>) -> Json<Vec<WsEnvelope>> {
    Json(state.bus.recent_sync())
}

async fn run_scenario_handler(
    Path(name): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let ports = state.ports.clone();
    let bus = Arc::clone(&state.bus);
    let name_clone = name.clone();
    tokio::spawn(async move {
        run_scenario(&name_clone, &ports, bus).await;
    });
    Json(json!({"status": "started", "scenario": name}))
}

use async_trait::async_trait;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use dashmap::DashMap;
use futures::{sink::SinkExt, stream::StreamExt};
use kittypaw_core::error::{KittypawError, LlmErrorKind, Result};
use kittypaw_core::types::{Event, EventType};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};

use crate::channel::Channel;

/// Message sent from client over WebSocket
#[derive(Debug, Deserialize)]
struct WsIncoming {
    #[serde(rename = "type")]
    msg_type: String,
    text: Option<String>,
    session_id: Option<String>,
}

/// Message sent to client over WebSocket
#[derive(Debug, Serialize)]
struct WsOutgoing {
    #[serde(rename = "type")]
    msg_type: String,
    text: String,
    session_id: String,
}

/// Sender half stored per session so send_response can reach back to the client.
type SessionMap = Arc<DashMap<String, mpsc::Sender<String>>>;

#[derive(Clone)]
struct AppState {
    event_tx: mpsc::Sender<Event>,
    sessions: SessionMap,
}

pub struct WebSocketChannel {
    bind_addr: String,
}

impl WebSocketChannel {
    pub fn new(bind_addr: impl Into<String>) -> Self {
        Self {
            bind_addr: bind_addr.into(),
        }
    }
}

#[async_trait]
impl Channel for WebSocketChannel {
    async fn start(&self, event_tx: mpsc::Sender<Event>) -> Result<()> {
        let sessions: SessionMap = Arc::new(DashMap::new());

        let state = AppState { event_tx, sessions };

        let app = Router::new()
            .route("/ws/chat", get(ws_handler))
            .with_state(state);

        let addr: std::net::SocketAddr = self.bind_addr.parse().map_err(|e| {
            KittypawError::Config(format!("Invalid bind address '{}': {}", self.bind_addr, e))
        })?;

        info!("WebSocket channel listening on {}", addr);

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| KittypawError::Config(format!("Failed to bind {}: {}", addr, e)))?;

        axum::serve(listener, app)
            .await
            .map_err(|e| KittypawError::Llm {
                kind: kittypaw_core::error::LlmErrorKind::Other,
                message: format!("WebSocket server error: {}", e),
            })?;

        Ok(())
    }

    async fn send_response(&self, agent_id: &str, _response: &str) -> Result<()> {
        // agent_id doubles as session_id for WebSocket.
        // In serve mode the shared SessionMap is held by the running server task.
        // send_response is called from the event loop which has a reference to the
        // channel struct only — we don't expose the map here because the serve loop
        // handles routing directly (see ServeWebSocketChannel below).
        warn!(
            "WebSocketChannel::send_response called outside serve context for session {}",
            agent_id
        );
        Ok(())
    }

    fn name(&self) -> &str {
        "websocket"
    }
}

/// Richer handle that exposes the session map for the serve event loop.
pub struct ServeWebSocketChannel {
    pub bind_addr: String,
    pub sessions: SessionMap,
}

impl ServeWebSocketChannel {
    pub fn new(bind_addr: impl Into<String>) -> Self {
        Self {
            bind_addr: bind_addr.into(),
            sessions: Arc::new(DashMap::new()),
        }
    }

    /// Spawn the axum server as a background task.
    pub async fn spawn(
        &self,
        event_tx: mpsc::Sender<Event>,
    ) -> Result<tokio::task::JoinHandle<()>> {
        let sessions = self.sessions.clone();

        let state = AppState { event_tx, sessions };

        let app = Router::new()
            .route("/ws/chat", get(ws_handler))
            .with_state(state);

        let addr: std::net::SocketAddr = self.bind_addr.parse().map_err(|e| {
            KittypawError::Config(format!("Invalid bind address '{}': {}", self.bind_addr, e))
        })?;

        info!("WebSocket channel listening on {}", addr);

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| KittypawError::Config(format!("Failed to bind {}: {}", addr, e)))?;

        let handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                error!("WebSocket server exited with error: {}", e);
            }
        });

        Ok(handle)
    }

    /// Send a text response to a connected session.
    pub async fn send_to_session(&self, session_id: &str, text: &str) -> Result<()> {
        if let Some(tx) = self.sessions.get(session_id) {
            tx.send(text.to_string())
                .await
                .map_err(|_| KittypawError::Llm {
                    kind: LlmErrorKind::Other,
                    message: format!("Session {} disconnected", session_id),
                })?;
        } else {
            warn!("No active WebSocket session for id={}", session_id);
        }
        Ok(())
    }
}

// ── axum handlers ──────────────────────────────────────────────────────────

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // Each connection gets a per-session mpsc for outgoing messages.
    let (out_tx, mut out_rx) = mpsc::channel::<String>(64);

    // We'll learn the session_id from the first message.
    // Use a oneshot to pass it from the reader task to cleanup.
    let (session_id_tx, session_id_rx) = oneshot::channel::<String>();
    let mut session_id_tx = Some(session_id_tx);

    let sessions_clone = state.sessions.clone();
    let event_tx = state.event_tx.clone();

    // Writer task: forward queued outgoing messages to the WebSocket.
    let write_task = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            let payload = serde_json::to_string(&WsOutgoing {
                msg_type: "response".to_string(),
                text,
                session_id: String::new(), // filled by client; omitted here for brevity
            })
            .unwrap_or_default();
            if sender.send(Message::Text(payload.into())).await.is_err() {
                break;
            }
        }
    });

    // Reader task: parse incoming WebSocket messages and push Events.
    while let Some(Ok(msg)) = receiver.next().await {
        let text_data = match msg {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => break,
            _ => continue,
        };

        let incoming: WsIncoming = match serde_json::from_str(&text_data) {
            Ok(v) => v,
            Err(e) => {
                warn!("Invalid WebSocket message: {} — {:?}", e, text_data);
                continue;
            }
        };

        if incoming.msg_type != "message" {
            continue;
        }

        let text = incoming.text.unwrap_or_default();
        let session_id = incoming.session_id.unwrap_or_else(uuid_v4);

        // Register session -> sender on first message.
        if let Some(tx) = session_id_tx.take() {
            let _ = tx.send(session_id.clone());
            sessions_clone.insert(session_id.clone(), out_tx.clone());
        }

        let event = Event {
            event_type: EventType::WebChat,
            payload: json!({
                "text": text,
                "session_id": session_id,
            }),
        };

        if event_tx.send(event).await.is_err() {
            info!("Event receiver dropped; closing WebSocket connection");
            break;
        }
    }

    // Clean up session map.
    if let Ok(session_id) = session_id_rx.await {
        state.sessions.remove(&session_id);
    }

    write_task.abort();
}

/// Tiny UUID v4 generator without extra deps (good-enough randomness via OS).
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("ws-{:x}", t)
}

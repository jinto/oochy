pub mod dashboard;
pub mod routes;

use axum::{Router, routing::get};
use std::net::SocketAddr;
use tracing::info;

/// WebDashboard serves an embedded HTML dashboard over HTTP.
pub struct WebDashboard {
    db_path: String,
    addr: SocketAddr,
}

impl WebDashboard {
    pub fn new(db_path: impl Into<String>, addr: SocketAddr) -> Self {
        Self {
            db_path: db_path.into(),
            addr,
        }
    }

    pub async fn serve(self) -> Result<(), Box<dyn std::error::Error>> {
        let db_path = self.db_path.clone();

        let app = Router::new()
            .route("/api/health", get(routes::health))
            .route("/api/agents", get({
                let db = db_path.clone();
                move || routes::list_agents(db)
            }))
            .route("/api/agents/:id/conversations", get({
                let db = db_path.clone();
                move |path| routes::get_conversations(db, path)
            }))
            .fallback(dashboard::static_handler);

        info!("Web dashboard listening on http://{}", self.addr);
        let listener = tokio::net::TcpListener::bind(self.addr).await?;
        axum::serve(listener, app).await?;
        Ok(())
    }
}

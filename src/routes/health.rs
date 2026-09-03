use axum::{routing::get, Json, Router};
use serde::Serialize;

#[derive(Serialize)]
pub struct Health {
    pub status: &'static str,
    pub service: &'static str,
    pub version: &'static str,
}

/// Ruta pública a propósito: la usan el healthcheck de Docker y el orquestador
/// para saber si el servicio está listo. No devuelve ningún dato de usuario.
pub fn router() -> Router {
    Router::new().route("/health", get(health))
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        service: "jobout-core-api",
        version: env!("CARGO_PKG_VERSION"),
    })
}

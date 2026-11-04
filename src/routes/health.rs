use axum::{routing::get, Json};
use serde::Serialize;

use super::table::{Access, RouteSpec};

#[derive(Serialize)]
pub struct Health {
    pub status: &'static str,
    pub service: &'static str,
    pub version: &'static str,
}

pub fn routes() -> Vec<RouteSpec> {
    vec![RouteSpec {
        path: "/health",
        methods: &["GET"],
        access: Access::Public(
            "Healthcheck de Docker y del orquestador; no devuelve datos de usuario.",
        ),
        handler: get(health),
    }]
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        service: "jobout-core-api",
        version: env!("CARGO_PKG_VERSION"),
    })
}

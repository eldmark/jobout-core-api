//! JobOut — core API.
//!
//! Este servicio es el único que habla con PostgreSQL y el único que ve PII.
//! Ver `documentation/arquitectura.md` y `documentation/security.md` en jobout-docs.

pub mod crypto;
pub mod jwt;
pub mod password;
pub mod routes;
pub mod state;

use axum::Router;
use tower_http::trace::TraceLayer;

/// Construye el router de la aplicación.
///
/// Vive acá y no en `main` para que las pruebas de integración puedan levantar
/// la app completa sin abrir un puerto.
pub fn app() -> Router {
    Router::new()
        .merge(routes::health::router())
        .layer(TraceLayer::new_for_http())
}

/// Construye la aplicación completa: las rutas de salud y las que necesitan base de datos.
///
/// `app()` queda como el router sin estado (solo `/health`); esta es la que arranca `main`.
pub fn app_with_state(state: state::AppState) -> Router {
    Router::new()
        .merge(routes::health::router())
        .merge(routes::auth::router().with_state(state))
        .layer(TraceLayer::new_for_http())
}

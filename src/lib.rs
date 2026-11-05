//! JobOut — core API.
//!
//! Este servicio es el único que habla con PostgreSQL y el único que ve PII.
//! Ver `documentation/arquitectura.md` y `documentation/security.md` en jobout-docs.

pub mod auth_user;
pub mod blob;
pub mod crypto;
pub mod cv;
pub mod jwt;
pub mod login_attempts;
pub mod password;
pub mod routes;
pub mod state;
pub mod telemetry;

use axum::Router;
use tower_http::trace::TraceLayer;

/// Construye la aplicación completa a partir de la tabla de rutas (`routes::table`), que es la
/// única forma de montar una ruta.
///
/// Vive acá y no en `main` para que las pruebas de integración puedan levantar la app completa
/// sin abrir un puerto.
pub fn app_with_state(state: state::AppState) -> Router {
    routes::table::build(routes::table::all(), state).layer(TraceLayer::new_for_http())
}

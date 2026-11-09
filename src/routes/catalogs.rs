//! `GET /api/v1/catalogs/{nombre}`: los catálogos cerrados que el perfil necesita para elegir
//! (skills, idiomas, niveles académicos, profesiones y jerarquías laborales).
//!
//! Devuelve solo los activos, ordenados por `sort_order` y luego `name`. El nombre de la URL
//! nunca llega al SQL: se traduce, por una lista cerrada, a un nombre de tabla fijo.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde::Serialize;
use serde_json::json;

use super::table::{Access, RouteSpec};
use crate::{auth_user::AuthUser, state::AppState};

pub fn routes() -> Vec<RouteSpec> {
    vec![RouteSpec {
        path: "/api/v1/catalogs/{nombre}",
        methods: &["GET"],
        access: Access::Authenticated,
        handler: get(list),
    }]
}

/// Nombre público -> consulta fija. Ninguna parte del nombre recibido se interpola.
fn consulta(nombre: &str) -> Option<&'static str> {
    Some(match nombre {
        "skills" => {
            "SELECT id::text, code, name FROM skills WHERE active ORDER BY sort_order, name"
        }
        "languages" => {
            "SELECT id::text, code, name FROM languages WHERE active ORDER BY sort_order, name"
        }
        "academic-levels" => {
            "SELECT id::text, code, name FROM academic_levels WHERE active \
             ORDER BY sort_order, name"
        }
        "professions" => {
            "SELECT id::text, code, name FROM professions WHERE active ORDER BY sort_order, name"
        }
        "job-hierarchies" => {
            "SELECT id::text, code, name FROM job_hierarchies WHERE active \
             ORDER BY sort_order, name"
        }
        _ => return None,
    })
}

enum ApiError {
    NotFound,
    Internal,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (estado, codigo, mensaje) = match self {
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "no_encontrado",
                "No existe ese catálogo.",
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "error_interno",
                "Algo salió mal de nuestro lado. Inténtalo de nuevo.",
            ),
        };
        (estado, Json(json!({"error": codigo, "message": mensaje}))).into_response()
    }
}

#[derive(Serialize)]
struct Item {
    id: String,
    code: String,
    name: String,
}

async fn list(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(nombre): Path<String>,
) -> Result<Json<Vec<Item>>, ApiError> {
    let sql = consulta(&nombre).ok_or(ApiError::NotFound)?;
    let filas = state
        .db
        .query_all(Statement::from_string(DbBackend::Postgres, sql))
        .await
        .map_err(|_| {
            tracing::error!(donde = "leer_catalogo", "error interno");
            ApiError::Internal
        })?;
    filas
        .iter()
        .map(|f| {
            Ok(Item {
                id: f.try_get_by_index(0)?,
                code: f.try_get_by_index(1)?,
                name: f.try_get_by_index(2)?,
            })
        })
        .collect::<Result<Vec<_>, sea_orm::DbErr>>()
        .map(Json)
        .map_err(|_| {
            tracing::error!(donde = "leer_fila_catalogo", "error interno");
            ApiError::Internal
        })
}

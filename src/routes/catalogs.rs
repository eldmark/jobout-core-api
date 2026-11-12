//! `GET /api/v1/catalogs/{nombre}`: los catálogos cerrados que el perfil y el formulario de una
//! oferta necesitan para elegir (skills, idiomas, niveles académicos, profesiones, jerarquías
//! laborales, países, estados, ciudades, rangos salariales, jornadas, tipos de contrato,
//! sectores y licencias).
//!
//! `states` y `cities` aceptan un padre opcional (`?country_id=` y `?state_id=`, un uuid): sin
//! él salen todos. El padre llega al SQL solo como parámetro enlazado y ya validado como uuid.
//!
//! Devuelve solo los activos, ordenados por `sort_order` y luego `name`. El nombre de la URL
//! nunca llega al SQL: se traduce, por una lista cerrada, a un nombre de tabla fijo.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;

use super::table::{Access, RouteSpec};
use crate::{auth_user::AuthUser, state::AppState};

pub fn routes() -> Vec<RouteSpec> {
    vec![RouteSpec {
        path: "/api/v1/catalogs/{nombre}",
        methods: &["GET"],
        access: Access::Authenticated,
        body_limit: None,
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
        "countries" => {
            "SELECT id::text, code, name FROM countries WHERE active ORDER BY sort_order, name"
        }
        "salary-ranges" => {
            "SELECT id::text, code, name FROM salary_ranges WHERE active ORDER BY sort_order, name"
        }
        "working-shifts" => {
            "SELECT id::text, code, name FROM working_shifts WHERE active \
             ORDER BY sort_order, name"
        }
        "contract-types" => {
            "SELECT id::text, code, name FROM contract_types WHERE active \
             ORDER BY sort_order, name"
        }
        "employment-sectors" => {
            "SELECT id::text, code, name FROM employment_sectors WHERE active \
             ORDER BY sort_order, name"
        }
        "driver-licenses" => {
            "SELECT id::text, code, name FROM driver_licenses WHERE active \
             ORDER BY sort_order, name"
        }
        _ => return None,
    })
}

/// Los catálogos con padre: (consulta sin filtro, consulta con filtro, parámetro). La segunda
/// recibe el uuid del padre como `$1`.
fn consulta_con_padre(nombre: &str) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match nombre {
        "states" => (
            "SELECT id::text, code, name FROM states WHERE active ORDER BY sort_order, name",
            "SELECT id::text, code, name FROM states WHERE active AND country_id = $1::uuid \
             ORDER BY sort_order, name",
            "country_id",
        ),
        "cities" => (
            "SELECT id::text, code, name FROM cities WHERE active ORDER BY sort_order, name",
            "SELECT id::text, code, name FROM cities WHERE active AND state_id = $1::uuid \
             ORDER BY sort_order, name",
            "state_id",
        ),
        _ => return None,
    })
}

enum ApiError {
    NotFound,
    BadParent,
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
            Self::BadParent => (
                StatusCode::BAD_REQUEST,
                "parametro_invalido",
                "El filtro por padre debe ser un identificador válido.",
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
    Query(filtros): Query<HashMap<String, String>>,
) -> Result<Json<Vec<Item>>, ApiError> {
    let sentencia = if let Some((todos, filtrada, parametro)) = consulta_con_padre(&nombre) {
        match filtros.get(parametro) {
            None => Statement::from_string(DbBackend::Postgres, todos),
            Some(padre) => {
                // Solo un uuid válido llega a la base.
                let padre = uuid::Uuid::parse_str(padre).map_err(|_| ApiError::BadParent)?;
                Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    filtrada,
                    [padre.to_string().into()],
                )
            }
        }
    } else {
        let sql = consulta(&nombre).ok_or(ApiError::NotFound)?;
        Statement::from_string(DbBackend::Postgres, sql)
    };
    let filas = state.db.query_all(sentencia).await.map_err(|_| {
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

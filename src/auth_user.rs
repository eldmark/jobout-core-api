//! Extractor `AuthUser`: quién hace la petición, resuelto siempre desde el token.
//!
//! Lee `Authorization: Bearer <jwt>`, valida firma, emisor, audiencia y vigencia con
//! `state.jwt`, y además **consulta la base**: el JWT dura 15 minutos y la cuenta puede haberse
//! borrado o suspendido dentro de ese lapso. Sin esa consulta, un token viejo seguiría
//! sirviendo tras `DELETE /me`.
//!
//! - Sin encabezado, esquema distinto, `Bearer` vacío o token inválido: 401.
//! - La cuenta ya no existe (o está marcada como borrada): 401.
//! - Cuenta `suspended`: 403.
//!
//! Todas las respuestas 401 son idénticas a propósito: no dicen si el token estaba mal firmado,
//! vencido o era de una cuenta borrada. Nada del token ni de la base se escribe en un log.

use axum::{
    extract::FromRequestParts,
    http::{header::AUTHORIZATION, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::json;

use crate::state::AppState;

/// Usuario autenticado y vigente. `id` es el UUID de `users.id` en texto.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: String,
}

pub enum AuthError {
    Unauthorized,
    Suspended,
    Internal,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (estado, codigo, mensaje) = match self {
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "no_autenticado",
                "Inicia sesión para continuar.",
            ),
            Self::Suspended => (
                StatusCode::FORBIDDEN,
                "cuenta_no_disponible",
                "Esta cuenta no puede usar el servicio.",
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

/// Saca el token de `Bearer <token>`. `None` si falta, el esquema es otro o el token viene vacío.
fn bearer(parts: &Parts) -> Option<&str> {
    let valor = parts.headers.get(AUTHORIZATION)?.to_str().ok()?;
    let token = valor.strip_prefix("Bearer ")?.trim();
    (!token.is_empty()).then_some(token)
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(parts).ok_or(AuthError::Unauthorized)?;
        let id = state
            .jwt
            .validate(token)
            .map_err(|_| AuthError::Unauthorized)?;

        let fila = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status::text FROM users WHERE id = $1::uuid AND deleted_at IS NULL",
                vec![id.clone().into()],
            ))
            .await
            .map_err(|_| {
                tracing::error!(donde = "auth_user_leer", "error interno");
                AuthError::Internal
            })?
            .ok_or(AuthError::Unauthorized)?;
        let estado: String = fila.try_get_by_index(0).map_err(|_| {
            tracing::error!(donde = "auth_user_estado", "error interno");
            AuthError::Internal
        })?;
        if estado == "suspended" {
            return Err(AuthError::Suspended);
        }
        Ok(Self { id })
    }
}

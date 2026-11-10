//! `GET /api/v1/me` y `DELETE /api/v1/me`: la cuenta de quien hace la petición.
//!
//! No existe ruta por id: el usuario sale siempre del token (`AuthUser`), así que nadie puede
//! ver ni borrar a otro. El borrado es real (el derecho a borrar de la política de privacidad):
//! se elimina la fila de `users` y las FK en cascada se llevan el resto. Queda una constancia en
//! `privacy_events` con el HMAC del email y nada más.
//!
//! Nada del email, la contraseña ni el token se escribe en un log; los errores internos se
//! registran por categoría, nunca con el texto del error de la base.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json,
};
use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement, TransactionTrait, Value};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::table::{Access, RouteSpec};
use crate::{auth_user::AuthUser, login_attempts, password::verify_password, state::AppState};

/// `GET` y `DELETE /api/v1/me`: protegidas. La autenticación la aplica la tabla de rutas al
/// montarlas; los handlers piden además `AuthUser` para saber **quién** es.
pub fn routes() -> Vec<RouteSpec> {
    vec![RouteSpec {
        path: "/api/v1/me",
        methods: &["GET", "DELETE"],
        access: Access::Authenticated,
        body_limit: None,
        handler: get(me).delete(delete_me),
    }]
}

enum ApiError {
    BadPassword,
    TooManyAttempts,
    Gone,
    Internal,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (estado, codigo, mensaje) = match self {
            Self::BadPassword => (
                StatusCode::FORBIDDEN,
                "contrasena_incorrecta",
                "La contraseña no es correcta.",
            ),
            Self::TooManyAttempts => (
                StatusCode::TOO_MANY_REQUESTS,
                "demasiados_intentos",
                "Demasiados intentos. Espera unos minutos e inténtalo de nuevo.",
            ),
            Self::Gone => (
                StatusCode::UNAUTHORIZED,
                "no_autenticado",
                "Inicia sesión para continuar.",
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

/// Registra el fallo por categoría, **nunca** con el texto del error de la base.
fn db(donde: &'static str) -> impl Fn(DbErr) -> ApiError {
    move |_| {
        tracing::error!(donde, "error interno");
        ApiError::Internal
    }
}

fn stmt(sql: &str, valores: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, valores)
}

#[derive(Serialize)]
struct PlanOut {
    code: String,
    name: String,
}

#[derive(Serialize)]
struct MeOut {
    id: String,
    first_name: String,
    last_name: String,
    status: String,
    email: String,
    plan: Option<PlanOut>,
    /// Códigos de rol, para que el frontend elija menús. Salen de la consulta de `AuthUser`.
    roles: Vec<String>,
    /// Códigos de permiso que dan esos roles. La autorización real la hace el backend.
    permissions: Vec<String>,
}

async fn me(State(state): State<AppState>, user: AuthUser) -> Result<Json<MeOut>, ApiError> {
    let fila = state
        .db
        .query_one(stmt(
            "SELECT u.id::text, u.first_name, u.last_name, u.status::text, u.email, \
                    p.code, p.name \
             FROM users u \
             LEFT JOIN user_subscriptions s ON s.user_id = u.id AND s.status = 'active' \
             LEFT JOIN plans p ON p.id = s.plan_id \
             WHERE u.id = $1::uuid AND u.deleted_at IS NULL",
            vec![user.id.clone().into()],
        ))
        .await
        .map_err(db("leer_me"))?
        .ok_or(ApiError::Gone)?;

    let email_cifrado: Vec<u8> = fila.try_get_by_index(4).map_err(db("leer_email"))?;
    let email = state.pii.decrypt(&email_cifrado).map_err(|_| {
        tracing::error!(donde = "descifrar_email", "error interno");
        ApiError::Internal
    })?;
    let codigo: Option<String> = fila.try_get_by_index(5).map_err(db("leer_plan"))?;
    let nombre: Option<String> = fila.try_get_by_index(6).map_err(db("leer_plan"))?;

    Ok(Json(MeOut {
        id: fila.try_get_by_index(0).map_err(db("leer_id"))?,
        first_name: fila.try_get_by_index(1).map_err(db("leer_nombre"))?,
        last_name: fila.try_get_by_index(2).map_err(db("leer_apellido"))?,
        status: fila.try_get_by_index(3).map_err(db("leer_estado"))?,
        email,
        plan: codigo
            .zip(nombre)
            .map(|(code, name)| PlanOut { code, name }),
        roles: user.roles,
        permissions: user.permissions,
    }))
}

#[derive(Deserialize)]
struct DeleteRequest {
    password: String,
}

async fn delete_me(
    State(state): State<AppState>,
    user: AuthUser,
    Json(entrada): Json<DeleteRequest>,
) -> Result<StatusCode, ApiError> {
    let fila = state
        .db
        .query_one(stmt(
            "SELECT u.email_hash, c.password_hash \
             FROM users u JOIN user_credentials c ON c.user_id = u.id \
             WHERE u.id = $1::uuid AND u.deleted_at IS NULL",
            vec![user.id.clone().into()],
        ))
        .await
        .map_err(db("leer_credenciales"))?
        .ok_or(ApiError::Gone)?;
    let email_hash: Vec<u8> = fila.try_get_by_index(0).map_err(db("leer_hash_email"))?;
    let password_hash: String = fila.try_get_by_index(1).map_err(db("leer_hash_clave"))?;

    // Mismo contador que el login: no se ganan intentos extra probando por otra ruta.
    let bloqueado = login_attempts::is_locked(&state.db, &email_hash)
        .await
        .map_err(db("leer_intentos"))?;
    if bloqueado {
        return Err(ApiError::TooManyAttempts);
    }

    // Argon2 es CPU puro: fuera del hilo asíncrono.
    let clave = entrada.password;
    let correcta = tokio::task::spawn_blocking(move || verify_password(&clave, &password_hash))
        .await
        .map_err(|_| {
            tracing::error!(donde = "verify_join", "error interno");
            ApiError::Internal
        })?;
    if !correcta {
        login_attempts::record_failure(&state.db, &email_hash)
            .await
            .map_err(db("registrar_fallo"))?;
        return Err(ApiError::BadPassword);
    }

    // Todo o nada: la constancia y el borrado van en la misma transacción.
    let txn = state.db.begin().await.map_err(db("begin"))?;

    // Las claves de los archivos del CV se leen ANTES de borrar: después las FK en cascada ya
    // se llevaron `cvs`. Van a la bandeja (cifradas de nuevo) en esta misma transacción, así que
    // la cuenta no puede quedar borrada sin que su archivo quede anotado para borrarse.
    let claves = txn
        .query_all(stmt(
            "SELECT c.storage_key FROM cvs c JOIN applicants a ON a.id = c.applicant_id \
             WHERE a.user_id = $1::uuid",
            vec![user.id.clone().into()],
        ))
        .await
        .map_err(db("leer_archivos"))?;
    let mut pendientes: Vec<(String, String)> = Vec::with_capacity(claves.len());
    for fila in claves {
        let cifrada: Vec<u8> = fila.try_get_by_index(0).map_err(db("leer_clave"))?;
        let clave = state.pii.decrypt(&cifrada).map_err(|_| {
            tracing::error!(donde = "descifrar_clave", "error interno");
            ApiError::Internal
        })?;
        let nueva = state.pii.encrypt(&clave).map_err(|_| {
            tracing::error!(donde = "cifrar_clave", "error interno");
            ApiError::Internal
        })?;
        let id = migration::new_id().to_string();
        txn.execute(stmt(
            "INSERT INTO pending_storage_deletions (id, storage_key) VALUES ($1::uuid, $2)",
            vec![id.clone().into(), nueva.into()],
        ))
        .await
        .map_err(db("encolar_archivo"))?;
        pendientes.push((id, clave));
    }

    txn.execute(stmt(
        "INSERT INTO privacy_events (id, subject_hash, kind) \
         VALUES ($1::uuid, $2, 'account_deleted')",
        vec![
            migration::new_id().to_string().into(),
            email_hash.clone().into(),
        ],
    ))
    .await
    .map_err(db("constancia"))?;
    login_attempts::clear(&txn, &email_hash)
        .await
        .map_err(db("limpiar_intentos"))?;
    let borradas = txn
        .execute(stmt(
            "DELETE FROM users WHERE id = $1::uuid",
            vec![user.id.into()],
        ))
        .await
        .map_err(db("borrar_usuario"))?
        .rows_affected();
    if borradas != 1 {
        // Otro DELETE concurrente ya se llevó la cuenta: sin constancia duplicada.
        return Err(ApiError::Gone);
    }
    txn.commit().await.map_err(db("commit"))?;

    // La cuenta ya no existe. Borrar los archivos es el mejor esfuerzo: lo que falle se queda
    // en la bandeja y lo reintenta `storage_cleanup`. Nunca cambia la respuesta ni se loguea
    // la clave.
    for (id, clave) in pendientes {
        if state.blobs.delete(&clave).await.is_err() {
            tracing::warn!(donde = "borrar_archivo", "archivo pendiente de borrar");
            continue;
        }
        if state
            .db
            .execute(stmt(
                "DELETE FROM pending_storage_deletions WHERE id = $1::uuid",
                vec![id.into()],
            ))
            .await
            .is_err()
        {
            tracing::warn!(donde = "quitar_de_bandeja", "no se pudo vaciar la bandeja");
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

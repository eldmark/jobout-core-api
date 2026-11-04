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
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement, TransactionTrait, Value};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{auth_user::AuthUser, password::verify_password, state::AppState};

/// Mismos valores que el límite de intentos de `login` (ver `routes::auth`).
const MAX_FAILURES: i32 = 5;
const WINDOW_MINUTES: i32 = 15;
const MAX_BODY_BYTES: usize = 8 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/me", get(me).delete(delete_me))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
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
            vec![user.id.into()],
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

    // Mismo bloqueo que el login (tabla `login_attempts`, por email_hash).
    let bloqueado = state
        .db
        .query_one(stmt(
            "SELECT failures >= $2 AND window_started_at > now() - make_interval(mins => $3) \
             FROM login_attempts WHERE email_hash = $1",
            vec![
                email_hash.clone().into(),
                MAX_FAILURES.into(),
                WINDOW_MINUTES.into(),
            ],
        ))
        .await
        .map_err(db("leer_intentos"))?
        .map(|f| f.try_get_by_index::<bool>(0).unwrap_or(false))
        .unwrap_or(false);
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
        state
            .db
            .execute(stmt(
                "INSERT INTO login_attempts (email_hash, failures, window_started_at) \
                 VALUES ($1, 1, now()) \
                 ON CONFLICT (email_hash) DO UPDATE SET \
                   failures = CASE WHEN login_attempts.window_started_at \
                                     < now() - make_interval(mins => $2) \
                                   THEN 1 ELSE login_attempts.failures + 1 END, \
                   window_started_at = CASE WHEN login_attempts.window_started_at \
                                     < now() - make_interval(mins => $2) \
                                   THEN now() ELSE login_attempts.window_started_at END, \
                   updated_at = now()",
                vec![email_hash.into(), WINDOW_MINUTES.into()],
            ))
            .await
            .map_err(db("registrar_fallo"))?;
        return Err(ApiError::BadPassword);
    }

    // Todo o nada: la constancia y el borrado van en la misma transacción.
    let txn = state.db.begin().await.map_err(db("begin"))?;
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
    txn.execute(stmt(
        "DELETE FROM login_attempts WHERE email_hash = $1",
        vec![email_hash.into()],
    ))
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
    Ok(StatusCode::NO_CONTENT)
}

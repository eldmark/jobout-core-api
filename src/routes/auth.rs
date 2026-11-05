//! Registro, ingreso, renovación y cierre de sesión. Autenticación propia del MVP (ver docs/working/fase1).
//!
//! Rutas públicas a propósito: son la puerta de entrada, no pueden exigir un token que todavía
//! no existe. Por eso se protegen de otra forma: límite de tamaño del cuerpo, límite de
//! intentos por email, respuestas que no distinguen «no existe» de «contraseña incorrecta», y
//! trabajo de Argon2 aunque el email no exista.
//!
//! `refresh` y `logout` también son públicas a propósito: se usan justo cuando el access token
//! ya expiró (o se perdió), así que no pueden exigirlo. Lo que las protege es que el refresh
//! token es un secreto de 256 bits que solo existe como hash en la base, que toda respuesta de
//! fallo es genérica (no distingue desconocido de expirado) y que reusar uno rotado revoca
//! todas las sesiones del usuario.
//!
//! Nada de lo que entra (email, contraseña) ni de lo que sale (tokens) se escribe en un log, y
//! los errores internos se registran por categoría, nunca con el texto del error de la base.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, SqlErr, Statement, TransactionTrait,
    Value,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::table::{Access, RouteSpec};
use crate::{
    jwt::ACCESS_TTL_SECS,
    login_attempts,
    password::{hash_password, validate_policy, verify_dummy, verify_password},
    state::AppState,
};

/// Vida del refresh token, en días.
const REFRESH_DAYS: i32 = 30;
const MAX_EMAIL_LEN: usize = 254;
const MAX_NAME_LEN: usize = 100;

/// Las rutas de sesión. Todas públicas, y cada una explica por qué (ver el encabezado del módulo).
pub fn routes() -> Vec<RouteSpec> {
    vec![
        RouteSpec {
            path: "/api/v1/auth/register",
            methods: &["POST"],
            access: Access::Public("Puerta de entrada: crea la cuenta, todavía no hay token."),
            body_limit: None,
            handler: post(register),
        },
        RouteSpec {
            path: "/api/v1/auth/login",
            methods: &["POST"],
            access: Access::Public(
                "Puerta de entrada: entrega el primer token a partir de la contraseña.",
            ),
            body_limit: None,
            handler: post(login),
        },
        RouteSpec {
            path: "/api/v1/auth/refresh",
            methods: &["POST"],
            access: Access::Public(
                "Se llama justo porque el access token venció; se autentica con el refresh token.",
            ),
            body_limit: None,
            handler: post(refresh),
        },
        RouteSpec {
            path: "/api/v1/auth/logout",
            methods: &["POST"],
            access: Access::Public(
                "Se autentica con el refresh token que revoca; debe poder cerrarse con el access vencido.",
            ),
            body_limit: None,
            handler: post(logout),
        },
    ]
}

// ---------------------------------------------------------------- errores

enum ApiError {
    Invalid,
    EmailUnavailable,
    BadCredentials,
    InvalidRefresh,
    TooManyAttempts,
    Forbidden,
    Internal,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (estado, codigo, mensaje) = match self {
            Self::Invalid => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "datos_invalidos",
                "Revisa los datos e inténtalo de nuevo.",
            ),
            Self::EmailUnavailable => (
                StatusCode::CONFLICT,
                "registro_no_disponible",
                "No pudimos crear la cuenta con esos datos.",
            ),
            Self::BadCredentials => (
                StatusCode::UNAUTHORIZED,
                "credenciales_invalidas",
                "El correo o la contraseña no son correctos.",
            ),
            Self::InvalidRefresh => (
                StatusCode::UNAUTHORIZED,
                "sesion_invalida",
                "La sesión no es válida. Inicia sesión de nuevo.",
            ),
            Self::TooManyAttempts => (
                StatusCode::TOO_MANY_REQUESTS,
                "demasiados_intentos",
                "Demasiados intentos. Espera unos minutos e inténtalo de nuevo.",
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "cuenta_no_disponible",
                "Esta cuenta no puede iniciar sesión.",
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

/// Registra el fallo por categoría. **Nunca** con el texto del error: el de la base puede
/// traer valores de la fila (hash del email, etc.).
fn interno(donde: &'static str) -> ApiError {
    tracing::error!(donde, "error interno");
    ApiError::Internal
}

fn db(donde: &'static str) -> impl Fn(DbErr) -> ApiError {
    move |_| interno(donde)
}

// ------------------------------------------------------------ entrada/salida

#[derive(Deserialize)]
struct RegisterRequest {
    email: String,
    password: String,
    first_name: String,
    last_name: String,
}

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Deserialize)]
struct RefreshRequest {
    refresh_token: String,
}

#[derive(Serialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    token_type: &'static str,
    expires_in: u64,
}

/// Quita espacios y pasa a minúsculas. Es la misma normalización que usa `email_hash`.
fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

fn email_is_valid(email: &str) -> bool {
    if email.is_empty() || email.len() > MAX_EMAIL_LEN || email.chars().any(char::is_whitespace) {
        return false;
    }
    let mut partes = email.split('@');
    let (Some(local), Some(dominio), None) = (partes.next(), partes.next(), partes.next()) else {
        return false;
    };
    !local.is_empty()
        && dominio.contains('.')
        && !dominio.starts_with('.')
        && !dominio.ends_with('.')
        && !dominio.contains("..")
}

fn name_is_valid(nombre: &str) -> bool {
    !nombre.trim().is_empty() && nombre.trim().chars().count() <= MAX_NAME_LEN
}

// ------------------------------------------------------------------- SQL

fn stmt(sql: &str, valores: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, valores)
}

fn sha256(texto: &str) -> Vec<u8> {
    Sha256::digest(texto.as_bytes()).to_vec()
}

/// Crea el par de tokens y guarda **solo el hash** del refresh token.
async fn issue_tokens(
    state: &AppState,
    conn: &impl ConnectionTrait,
    user_id: &str,
) -> Result<TokenResponse, ApiError> {
    use aes_gcm::aead::{rand_core::RngCore, OsRng};

    let access_token = state
        .jwt
        .issue(user_id)
        .map_err(|_| interno("emitir_jwt"))?;

    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let refresh_token = URL_SAFE_NO_PAD.encode(bytes);

    conn.execute(stmt(
        "INSERT INTO refresh_tokens (id, user_id, token_hash, expires_at) \
         VALUES ($1::uuid, $2::uuid, $3, now() + make_interval(days => $4))",
        vec![
            migration::new_id().to_string().into(),
            user_id.to_string().into(),
            sha256(&refresh_token).into(),
            REFRESH_DAYS.into(),
        ],
    ))
    .await
    .map_err(db("guardar_refresh"))?;

    Ok(TokenResponse {
        access_token,
        refresh_token,
        token_type: "Bearer",
        expires_in: ACCESS_TTL_SECS,
    })
}

// -------------------------------------------------------------- registro

async fn register(
    State(state): State<AppState>,
    Json(entrada): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<TokenResponse>), ApiError> {
    let email = normalize_email(&entrada.email);
    if !email_is_valid(&email)
        || !name_is_valid(&entrada.first_name)
        || !name_is_valid(&entrada.last_name)
        || validate_policy(&entrada.password).is_err()
    {
        return Err(ApiError::Invalid);
    }

    let email_cifrado = state.pii.encrypt(&email).map_err(|_| interno("cifrar"))?;
    let email_hash = state.pii.email_hash(&email);

    // Argon2 es CPU puro: fuera del hilo asíncrono para no frenar las demás peticiones.
    let clave = entrada.password;
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&clave))
        .await
        .map_err(|_| interno("hash_join"))?
        .map_err(|_| interno("hash"))?;

    let txn = state.db.begin().await.map_err(db("begin"))?;
    let user_id = match crear_cuenta(
        &txn,
        &email_cifrado,
        &email_hash,
        &entrada.first_name,
        &entrada.last_name,
        &password_hash,
    )
    .await
    {
        Ok(id) => id,
        // Dos registros del mismo email: el segundo pierde contra el índice único.
        Err(e) if matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => {
            return Err(ApiError::EmailUnavailable);
        }
        Err(_) => return Err(interno("crear_cuenta")),
    };
    let tokens = issue_tokens(&state, &txn, &user_id).await?;
    txn.commit().await.map_err(db("commit"))?;

    Ok((StatusCode::CREATED, Json(tokens)))
}

async fn crear_cuenta(
    txn: &DatabaseTransaction,
    email_cifrado: &[u8],
    email_hash: &[u8],
    nombre: &str,
    apellido: &str,
    password_hash: &str,
) -> Result<String, DbErr> {
    let user_id = migration::new_id().to_string();

    txn.execute(stmt(
        "INSERT INTO users (id, email, email_hash, first_name, last_name, status) \
         VALUES ($1::uuid, $2, $3, $4, $5, 'active')",
        vec![
            user_id.clone().into(),
            email_cifrado.to_vec().into(),
            email_hash.to_vec().into(),
            nombre.trim().to_string().into(),
            apellido.trim().to_string().into(),
        ],
    ))
    .await?;
    txn.execute(stmt(
        "INSERT INTO user_credentials (user_id, password_hash) VALUES ($1::uuid, $2)",
        vec![user_id.clone().into(), password_hash.to_string().into()],
    ))
    .await?;
    txn.execute(stmt(
        "INSERT INTO applicants (id, user_id) VALUES ($1::uuid, $2::uuid)",
        vec![
            migration::new_id().to_string().into(),
            user_id.clone().into(),
        ],
    ))
    .await?;
    txn.execute(stmt(
        "INSERT INTO user_roles (user_id, role_id) \
         SELECT $1::uuid, id FROM roles WHERE code = 'applicant'",
        vec![user_id.clone().into()],
    ))
    .await?;
    txn.execute(stmt(
        "INSERT INTO user_subscriptions (id, user_id, plan_id, status, source) \
         SELECT $1::uuid, $2::uuid, id, 'active', 'free' FROM plans WHERE code = 'free'",
        vec![
            migration::new_id().to_string().into(),
            user_id.clone().into(),
        ],
    ))
    .await?;
    Ok(user_id)
}

// ----------------------------------------------------------------- login

async fn login(
    State(state): State<AppState>,
    Json(entrada): Json<LoginRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let email = normalize_email(&entrada.email);
    let email_hash = state.pii.email_hash(&email);

    // 1. ¿Bloqueado? Antes de mirar la contraseña: ni la correcta pasa durante el bloqueo.
    let bloqueado = login_attempts::is_locked(&state.db, &email_hash)
        .await
        .map_err(db("leer_intentos"))?;
    if bloqueado {
        return Err(ApiError::TooManyAttempts);
    }

    // 2. Usuario y hash. Si no existe se verifica contra un hash ficticio: mismo costo.
    let fila = state
        .db
        .query_one(stmt(
            "SELECT u.id::text, u.status::text, c.password_hash \
             FROM users u JOIN user_credentials c ON c.user_id = u.id \
             WHERE u.email_hash = $1 AND u.deleted_at IS NULL",
            vec![email_hash.clone().into()],
        ))
        .await
        .map_err(db("leer_usuario"))?;

    let clave = entrada.password;
    let (cuenta, correcta) = match fila {
        Some(f) => {
            let id: String = f.try_get_by_index(0).map_err(db("leer_id"))?;
            let estado: String = f.try_get_by_index(1).map_err(db("leer_estado"))?;
            let hash: String = f.try_get_by_index(2).map_err(db("leer_hash"))?;
            let ok = tokio::task::spawn_blocking(move || verify_password(&clave, &hash))
                .await
                .map_err(|_| interno("verify_join"))?;
            (Some((id, estado)), ok)
        }
        None => {
            tokio::task::spawn_blocking(move || verify_dummy(&clave))
                .await
                .map_err(|_| interno("dummy_join"))?;
            (None, false)
        }
    };

    // 3. Fallo: se cuenta, exista o no el email, y la respuesta es siempre la misma.
    let (Some((user_id, estado)), true) = (cuenta, correcta) else {
        login_attempts::record_failure(&state.db, &email_hash)
            .await
            .map_err(db("registrar_fallo"))?;
        return Err(ApiError::BadCredentials);
    };

    if estado == "suspended" {
        return Err(ApiError::Forbidden);
    }

    // 4. Éxito: se reinicia el contador y se entregan los tokens.
    login_attempts::clear(&state.db, &email_hash)
        .await
        .map_err(db("reiniciar_intentos"))?;

    Ok(Json(issue_tokens(&state, &state.db, &user_id).await?))
}

// --------------------------------------------------------------- refresh

/// Rota el refresh token: el presentado queda revocado y se entrega un par nuevo.
///
/// Todo ocurre en una transacción. El `UPDATE ... WHERE revoked_at IS NULL` es la compuerta
/// atómica: con dos peticiones simultáneas la segunda espera el bloqueo de la fila y, al
/// reevaluar, ya no la encuentra viva; así solo una rota y la otra cuenta como reuso.
async fn refresh(
    State(state): State<AppState>,
    Json(entrada): Json<RefreshRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let hash = sha256(&entrada.refresh_token);
    let txn = state.db.begin().await.map_err(db("begin"))?;

    let rotado = txn
        .query_one(stmt(
            "UPDATE refresh_tokens SET revoked_at = now(), updated_at = now() \
             WHERE token_hash = $1 AND revoked_at IS NULL AND expires_at > now() \
             RETURNING user_id::text",
            vec![hash.clone().into()],
        ))
        .await
        .map_err(db("rotar_refresh"))?;

    let Some(fila) = rotado else {
        // No sirvió. Si existe y estaba revocado, alguien lo está reusando: se cierran todas
        // las sesiones del usuario y se confirma ANTES de responder 401. Si no existe o
        // simplemente expiró, no hay nada que revocar.
        let revocado = txn
            .query_one(stmt(
                "SELECT user_id::text FROM refresh_tokens \
                 WHERE token_hash = $1 AND revoked_at IS NOT NULL",
                vec![hash.into()],
            ))
            .await
            .map_err(db("buscar_reuso"))?;
        if let Some(fila) = revocado {
            let user_id: String = fila.try_get_by_index(0).map_err(db("leer_usuario_reuso"))?;
            txn.execute(stmt(
                "UPDATE refresh_tokens SET revoked_at = now(), updated_at = now() \
                 WHERE user_id = $1::uuid AND revoked_at IS NULL",
                vec![user_id.into()],
            ))
            .await
            .map_err(db("revocar_sesiones"))?;
            tracing::warn!("reuso de refresh token: sesiones revocadas");
        }
        txn.commit().await.map_err(db("commit"))?;
        return Err(ApiError::InvalidRefresh);
    };
    let user_id: String = fila.try_get_by_index(0).map_err(db("leer_usuario"))?;

    let estado = txn
        .query_one(stmt(
            "SELECT status::text FROM users WHERE id = $1::uuid AND deleted_at IS NULL",
            vec![user_id.clone().into()],
        ))
        .await
        .map_err(db("leer_estado"))?
        .map(|f| f.try_get_by_index::<String>(0))
        .transpose()
        .map_err(db("leer_estado"))?;
    match estado.as_deref() {
        Some("suspended") => return Err(ApiError::Forbidden), // la transacción se descarta
        None => return Err(ApiError::InvalidRefresh),
        Some(_) => {}
    }

    let tokens = issue_tokens(&state, &txn, &user_id).await?;
    txn.commit().await.map_err(db("commit"))?;
    Ok(Json(tokens))
}

// ---------------------------------------------------------------- logout

/// Revoca ese refresh token y nada más. Idempotente: desconocido o ya revocado también es
/// 204, para no revelar si el token existió.
async fn logout(
    State(state): State<AppState>,
    Json(entrada): Json<RefreshRequest>,
) -> Result<StatusCode, ApiError> {
    state
        .db
        .execute(stmt(
            "UPDATE refresh_tokens SET revoked_at = now(), updated_at = now() \
             WHERE token_hash = $1 AND revoked_at IS NULL",
            vec![sha256(&entrada.refresh_token).into()],
        ))
        .await
        .map_err(db("revocar_refresh"))?;
    Ok(StatusCode::NO_CONTENT)
}

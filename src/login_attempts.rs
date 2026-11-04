//! Límite de intentos fallidos de contraseña, por email (su HMAC, nunca el email).
//!
//! Lo usan `login` y `DELETE /me`, que comparten el mismo contador: un atacante no gana
//! intentos extra probando contraseñas por una ruta distinta. La tabla (`login_attempts`) no
//! tiene FK a `users` a propósito: también cuenta emails que no existen, para que el bloqueo no
//! revele cuáles tienen cuenta.

use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement};

/// Fallos seguidos, dentro de la ventana, que bloquean al email.
pub const MAX_FAILURES: i32 = 5;
/// Ventana de conteo y de bloqueo, en minutos.
pub const WINDOW_MINUTES: i32 = 15;

fn stmt(sql: &str, valores: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, valores)
}

/// ¿Está bloqueado ahora? Se consulta **antes** de mirar la contraseña: durante el bloqueo ni la
/// correcta entra.
pub async fn is_locked(conn: &impl ConnectionTrait, email_hash: &[u8]) -> Result<bool, DbErr> {
    let fila = conn
        .query_one(stmt(
            "SELECT failures >= $2 AND window_started_at > now() - make_interval(mins => $3) \
             FROM login_attempts WHERE email_hash = $1",
            vec![
                email_hash.to_vec().into(),
                MAX_FAILURES.into(),
                WINDOW_MINUTES.into(),
            ],
        ))
        .await?;
    Ok(fila
        .map(|f| f.try_get_by_index::<bool>(0).unwrap_or(false))
        .unwrap_or(false))
}

/// Suma un fallo. Pasada la ventana, la cuenta se reinicia en 1. Es una sola sentencia atómica
/// (`INSERT … ON CONFLICT`), así que fallos simultáneos no se pierden.
pub async fn record_failure(conn: &impl ConnectionTrait, email_hash: &[u8]) -> Result<(), DbErr> {
    conn.execute(stmt(
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
        vec![email_hash.to_vec().into(), WINDOW_MINUTES.into()],
    ))
    .await?;
    Ok(())
}

/// Borra el contador (ingreso correcto o cuenta borrada). No falla si no había nada.
pub async fn clear(conn: &impl ConnectionTrait, email_hash: &[u8]) -> Result<(), DbErr> {
    conn.execute(stmt(
        "DELETE FROM login_attempts WHERE email_hash = $1",
        vec![email_hash.to_vec().into()],
    ))
    .await?;
    Ok(())
}

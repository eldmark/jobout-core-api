//! Cuota mensual de sesiones de entrevista por plan (Fase 3, paso 9).
//!
//! - El tope vive en `plans.monthly_simulations` (null = sin tope); aquí no hay constantes por
//!   plan. Sin suscripción activa (ninguna, vencida o cancelada) se aplica el tope de **Gratis**:
//!   nunca «sin tope».
//! - Se cuenta lo **iniciado**: las filas de `usage_ledger` con `operation = 'interview_session'`
//!   del mes calendario UTC. Abandonar o borrar la sesión no borra su fila, así que no devuelve
//!   la cuota.
//! - La comprobación y la fila de uso se hacen dentro de la transacción que crea la sesión
//!   ([`crate::interview_store::create_session`]), con un candado por persona: dos peticiones
//!   simultáneas no pueden pasar de largo el tope.
//! - `ENFORCE_PLAN_LIMITS=false` salta **solo** la comprobación; el uso se sigue registrando.
//! - El reloj se inyecta (`QuotaPolicy::now_micros`): las pruebas fijan el instante, nunca duermen.

use sea_orm::{ConnectionTrait, DbBackend, Statement, Value};
use uuid::Uuid;

use crate::interview_store::StoreError;

/// `usage_ledger.operation` de una sesión de entrevista iniciada.
pub const OPERATION: &str = "interview_session";
/// `usage_ledger.provider` del motor determinista. El motor de IA traerá el suyo, con costo real.
pub const PROVIDER: &str = "deterministic";

/// Cómo aplicar la cuota en una creación.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaPolicy {
    /// Si es falso se salta la comprobación (no el registro del uso).
    pub enforce: bool,
    /// «Ahora», en microsegundos Unix. Decide el mes que se cuenta y `occurred_at`.
    pub now_micros: i64,
}

impl QuotaPolicy {
    /// La política de producción: el interruptor del entorno y el reloj del sistema.
    pub fn current() -> Self {
        let now_micros = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX));
        Self {
            enforce: enforce_plan_limits(),
            now_micros,
        }
    }
}

/// `ENFORCE_PLAN_LIMITS`: activa por defecto; solo un `false` o `0` explícito la apaga.
pub fn enforce_from_env_value(valor: Option<&str>) -> bool {
    match valor.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) => v != "false" && v != "0",
        None => true,
    }
}

/// El valor de `ENFORCE_PLAN_LIMITS` en el entorno del proceso.
pub fn enforce_plan_limits() -> bool {
    enforce_from_env_value(std::env::var("ENFORCE_PLAN_LIMITS").ok().as_deref())
}

fn stmt(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

/// `now_micros` como `timestamptz`, exacto (aritmética entera, sin flotantes).
const AHORA: &str = "(timestamptz 'epoch' + $2::bigint * interval '1 microsecond')";

/// Serializa a las creaciones de la misma persona hasta el fin de la transacción.
pub async fn lock_user<C: ConnectionTrait>(txn: &C, user: Uuid) -> Result<(), StoreError> {
    txn.execute(stmt(
        "SELECT pg_advisory_xact_lock(hashtextextended('interview_quota:' || $1::text, 0))",
        vec![user.into()],
    ))
    .await?;
    Ok(())
}

/// El tope mensual vigente de la persona: `Some(n)`, o `None` si su plan no tiene tope.
async fn monthly_limit<C: ConnectionTrait>(
    txn: &C,
    user: Uuid,
    now_micros: i64,
) -> Result<Option<i32>, StoreError> {
    let sql = format!(
        "SELECT p.monthly_simulations FROM user_subscriptions s \
         JOIN plans p ON p.id = s.plan_id \
         WHERE s.user_id = $1 AND s.status = 'active' \
           AND (s.expires_at IS NULL OR s.expires_at > {AHORA})"
    );
    let propio = txn
        .query_one(stmt(&sql, vec![user.into(), now_micros.into()]))
        .await?;
    let fila = match propio {
        Some(fila) => fila,
        // Sin suscripción activa: Gratis, nunca ilimitado.
        None => txn
            .query_one(stmt(
                "SELECT monthly_simulations FROM plans WHERE code = 'free'",
                vec![],
            ))
            .await?
            .ok_or(StoreError::Corrupt)?,
    };
    Ok(fila.try_get_by_index(0)?)
}

/// `Some(renews_at)` si la persona ya usó su tope del mes de `now_micros`; `None` si puede
/// iniciar otra. `renews_at` es el primer instante del mes siguiente, UTC.
pub async fn exceeded<C: ConnectionTrait>(
    txn: &C,
    user: Uuid,
    now_micros: i64,
) -> Result<Option<String>, StoreError> {
    let Some(limite) = monthly_limit(txn, user, now_micros).await? else {
        return Ok(None);
    };
    let sql = format!(
        "WITH t AS (SELECT date_trunc('month', {AHORA} AT TIME ZONE 'UTC') AS m) \
         SELECT (SELECT count(*) FROM usage_ledger l, t \
                 WHERE l.user_id = $1 AND l.operation = $3 \
                   AND l.occurred_at >= t.m AT TIME ZONE 'UTC' \
                   AND l.occurred_at < (t.m + interval '1 month') AT TIME ZONE 'UTC'), \
                (SELECT to_char(m + interval '1 month', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') FROM t)"
    );
    let fila = txn
        .query_one(stmt(
            &sql,
            vec![user.into(), now_micros.into(), OPERATION.into()],
        ))
        .await?
        .ok_or(StoreError::Corrupt)?;
    let usadas: i64 = fila.try_get_by_index(0)?;
    let renueva: String = fila.try_get_by_index(1)?;
    Ok((usadas >= i64::from(limite)).then_some(renueva))
}

/// Anota el uso de una sesión iniciada: una unidad, sin costo (el motor es determinista).
pub async fn record_session<C: ConnectionTrait>(
    txn: &C,
    user: Uuid,
    session: Uuid,
    now_micros: i64,
) -> Result<(), StoreError> {
    let sql = format!(
        "INSERT INTO usage_ledger \
            (id, user_id, interview_session_id, provider, operation, units, cost_micros, occurred_at) \
         VALUES ($1, $3, $4, $5, $6, 1, 0, {AHORA})"
    );
    txn.execute(stmt(
        &sql,
        vec![
            migration::new_id().into(),
            now_micros.into(),
            user.into(),
            session.into(),
            PROVIDER.into(),
            OPERATION.into(),
        ],
    ))
    .await?;
    Ok(())
}

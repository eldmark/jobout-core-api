//! «Empresa verificada y con membresía vigente»: el control que comparten publicar, reanudar y el
//! panel del reclutador (api.md, «Efectos de la vigencia»). Se consulta en **cada petición**, no
//! en una tarea: así quitar la verificación o vencer la membresía surte efecto de inmediato.
//!
//! - Sin verificar gana siempre sobre una membresía vencida.
//! - «Vigente» es `start_date <= hoy <= expiration_date` (fecha UTC) y `status = 'active'`.
//! - Una empresa que no existe (o está borrada) no pasa, y se trata como no verificada.
//! - «Hoy» se puede inyectar ([`check_on`]); [`check`] usa la fecha UTC de la base.

use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement};
use uuid::Uuid;

/// La misma regla como condición SQL para los listados (alias `c` = `companies`): empresa viva,
/// activa, verificada y con una membresía vigente hoy (fecha UTC). Es lo que decide qué ofertas
/// ven los candidatos; se evalúa dentro de la consulta para no hacer una por oferta.
pub const VISIBLE_COMPANY_SQL: &str = "c.deleted_at IS NULL AND c.active \
     AND c.verified_at IS NOT NULL \
     AND EXISTS (SELECT 1 FROM memberships m \
                 WHERE m.company_id = c.id AND m.status = 'active' \
                   AND m.start_date <= (now() AT TIME ZONE 'UTC')::date \
                   AND (now() AT TIME ZONE 'UTC')::date <= m.expiration_date)";

/// Por qué una empresa no puede publicar ni usar el panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// 403 `empresa_no_verificada`.
    NotVerified,
    /// 403 `membresia_vencida`.
    MembershipExpired,
}

/// Con la fecha UTC de hoy de la base.
pub async fn check(conn: &impl ConnectionTrait, company: Uuid) -> Result<Result<(), Gate>, DbErr> {
    run(conn, company, None).await
}

/// Con «hoy» dado como `YYYY-MM-DD` (para probar sin reloj).
pub async fn check_on(
    conn: &impl ConnectionTrait,
    company: Uuid,
    today: &str,
) -> Result<Result<(), Gate>, DbErr> {
    run(conn, company, Some(today.to_string())).await
}

async fn run(
    conn: &impl ConnectionTrait,
    company: Uuid,
    today: Option<String>,
) -> Result<Result<(), Gate>, DbErr> {
    let fila = conn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "WITH hoy AS (SELECT COALESCE($2::text::date, (now() AT TIME ZONE 'UTC')::date) AS d) \
             SELECT c.verified_at IS NOT NULL, \
                    EXISTS (SELECT 1 FROM memberships m, hoy \
                            WHERE m.company_id = c.id AND m.status = 'active' \
                              AND m.start_date <= hoy.d AND hoy.d <= m.expiration_date) \
             FROM companies c WHERE c.id = $1 AND c.deleted_at IS NULL",
            vec![company.into(), today.into()],
        ))
        .await?;
    let Some(fila) = fila else {
        return Ok(Err(Gate::NotVerified));
    };
    let verificada: bool = fila.try_get_by_index(0)?;
    let vigente: bool = fila.try_get_by_index(1)?;
    Ok(match (verificada, vigente) {
        (false, _) => Err(Gate::NotVerified),
        (true, false) => Err(Gate::MembershipExpired),
        (true, true) => Ok(()),
    })
}

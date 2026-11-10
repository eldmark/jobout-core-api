//! Ofertas de empleo del reclutador (Fase 5a, paso 4): validación pura (`input`), SQL (`store`)
//! y el cierre automático por fecha de contratación.
//!
//! Las rutas están en `routes::offers`. Contrato: `api.md`, «Ofertas del reclutador».

pub mod input;
pub mod store;

use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement};

/// OF9: cierra las ofertas `published`/`paused` cuya `hiring_date` ya pasó (fecha UTC de
/// `now_micros`) y fija `closed_at = now_micros`. Devuelve cuántas cerró.
///
/// El reloj entra como parámetro (microsegundos Unix): las pruebas fijan el instante, nunca
/// duermen. Es un solo `UPDATE`: si un reclutador cierra o pausa a la vez, Postgres reevalúa la
/// condición sobre la fila ya bloqueada, así que una cerrada a mano no se cierra dos veces ni
/// pierde su `closed_at`. No toca las aplicaciones (siguen mientras dure la retención).
pub async fn close_expired<C: ConnectionTrait>(db: &C, now_micros: i64) -> Result<u64, DbErr> {
    let r = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE job_offers \
             SET status = 'closed', \
                 closed_at = (timestamptz 'epoch' + $1::bigint * interval '1 microsecond'), \
                 updated_at = now() \
             WHERE status IN ('published', 'paused') \
               AND hiring_date < \
                   ((timestamptz 'epoch' + $1::bigint * interval '1 microsecond') \
                    AT TIME ZONE 'UTC')::date",
            vec![now_micros.into()],
        ))
        .await?;
    Ok(r.rows_affected())
}

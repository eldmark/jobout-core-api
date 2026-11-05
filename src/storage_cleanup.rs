//! Reintento del borrado de archivos: vacía `pending_storage_deletions`.
//!
//! Al borrar una cuenta, las claves de sus archivos se anotan en la bandeja (cifradas) y se
//! intenta borrarlas. Lo que el almacén no pudo borrar queda aquí, y este módulo lo reintenta.
//!
//! - Borrar es idempotente (`BlobStore::delete`): un archivo que ya no existe cuenta como éxito.
//! - Éxito: la fila se elimina; la bandeja no conserva nada de quien se fue.
//! - Fallo: `attempts += 1` y `last_attempt_at = now()`.
//! - Revisión humana: una fila con `attempts >= MAX_ATTEMPTS` deja de reintentarse y se queda en
//!   la tabla. Se encuentra con `WHERE attempts >= 10`. Al cruzar el umbral se registra un
//!   error con el id de la fila (nunca la clave del archivo).

use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement};

use crate::state::AppState;

/// Intentos tras los cuales una clave se marca para revisión humana y no se reintenta más.
pub const MAX_ATTEMPTS: i32 = 10;
/// Filas por lote.
pub const BATCH_SIZE: u64 = 50;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CleanupReport {
    /// Archivos borrados (o que ya no existían); su fila salió de la bandeja.
    pub deleted: u32,
    /// Intentos fallidos en esta pasada.
    pub failed: u32,
    /// Filas que en esta pasada llegaron al máximo de intentos.
    pub flagged_for_review: u32,
}

fn stmt(sql: &str, valores: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, valores)
}

/// Una pasada por la bandeja, en lotes. Cada fila se intenta como mucho una vez por pasada.
pub async fn process_pending_deletions(state: &AppState) -> Result<CleanupReport, DbErr> {
    let mut informe = CleanupReport::default();
    let mut ultimo = String::from("00000000-0000-0000-0000-000000000000");
    loop {
        let lote = state
            .db
            .query_all(stmt(
                "SELECT id::text, storage_key FROM pending_storage_deletions \
                 WHERE attempts < $1 AND id > $2::uuid ORDER BY id LIMIT $3",
                vec![
                    MAX_ATTEMPTS.into(),
                    ultimo.clone().into(),
                    (BATCH_SIZE as i64).into(),
                ],
            ))
            .await?;
        if lote.is_empty() {
            return Ok(informe);
        }
        for fila in lote {
            let id: String = fila.try_get_by_index(0)?;
            let cifrada: Vec<u8> = fila.try_get_by_index(1)?;
            ultimo = id.clone();

            let borrado = match state.pii.decrypt(&cifrada) {
                Ok(clave) => state.blobs.delete(&clave).await.is_ok(),
                Err(_) => false,
            };
            if borrado {
                state
                    .db
                    .execute(stmt(
                        "DELETE FROM pending_storage_deletions WHERE id = $1::uuid",
                        vec![id.into()],
                    ))
                    .await?;
                informe.deleted += 1;
                continue;
            }

            let intentos: i32 = state
                .db
                .query_one(stmt(
                    "UPDATE pending_storage_deletions \
                     SET attempts = attempts + 1, last_attempt_at = now(), updated_at = now() \
                     WHERE id = $1::uuid RETURNING attempts",
                    vec![id.clone().into()],
                ))
                .await?
                .map(|f| f.try_get_by_index(0))
                .transpose()?
                .unwrap_or(MAX_ATTEMPTS);
            informe.failed += 1;
            if intentos >= MAX_ATTEMPTS {
                informe.flagged_for_review += 1;
                tracing::error!(
                    pending_id = %id,
                    intentos,
                    "borrado de archivo pendiente: requiere revisión humana"
                );
            }
        }
    }
}

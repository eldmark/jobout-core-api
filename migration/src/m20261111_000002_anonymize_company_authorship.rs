//! Borrado de cuenta ampliado del lado empresa (Fase 5a, paso 10, BO3): las columnas de autoría
//! que quedaban sin FK pasan a `REFERENCES users (id) ON DELETE SET NULL`.
//!
//! Antes de esto, `companies.updated_by` (quien editó o verificó la empresa),
//! `memberships.created_by`/`updated_by` (el admin de la plataforma que fijó la membresía) y
//! `company_recruiters.created_by`/`updated_by` (quien dio de alta o de baja a un reclutador)
//! conservaban el id de una cuenta ya borrada. Con la FK, la base los deja en `null` en la misma
//! transacción del borrado, como ya pasa con `job_offers.created_by` y
//! `job_application_events.changed_by`: la decisión queda, sin autor.
//!
//! `up` primero limpia los ids que ya no existen (si no, la FK no se puede crear); `down` quita
//! las FK y deja los valores como estén.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// `(tabla, columna)`: constantes de este archivo, nunca datos recibidos.
const COLUMNAS: [(&str, &str); 5] = [
    ("companies", "updated_by"),
    ("memberships", "created_by"),
    ("memberships", "updated_by"),
    ("company_recruiters", "created_by"),
    ("company_recruiters", "updated_by"),
];

fn restriccion(tabla: &str, col: &str) -> String {
    format!("{tabla}_{col}_fkey")
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for (tabla, col) in COLUMNAS {
            db.execute_unprepared(&format!(
                "UPDATE {tabla} SET {col} = NULL \
                 WHERE {col} IS NOT NULL AND NOT EXISTS (SELECT 1 FROM users u WHERE u.id = {tabla}.{col}); \
                 ALTER TABLE {tabla} ADD CONSTRAINT {} \
                     FOREIGN KEY ({col}) REFERENCES users (id) ON DELETE SET NULL; \
                 CREATE INDEX {tabla}_{col}_idx ON {tabla} ({col}) WHERE {col} IS NOT NULL;",
                restriccion(tabla, col)
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for (tabla, col) in COLUMNAS {
            db.execute_unprepared(&format!(
                "DROP INDEX IF EXISTS {tabla}_{col}_idx; \
                 ALTER TABLE {tabla} DROP CONSTRAINT IF EXISTS {};",
                restriccion(tabla, col)
            ))
            .await?;
        }
        Ok(())
    }
}

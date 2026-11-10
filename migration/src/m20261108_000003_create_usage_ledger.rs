//! `usage_ledger`: una fila por cada unidad de uso con costo (hoy, cada sesión de entrevista
//! iniciada). Sirve para dos cosas: contar la cuota mensual del plan y conocer el costo agregado.
//!
//! Dos decisiones de privacidad, ambas `ON DELETE SET NULL`:
//!
//! - `user_id`: al borrar la cuenta la fila **sobrevive** sin el id de la persona, para que el
//!   costo agregado no se pierda (HU-05).
//! - `interview_session_id`: borrar una sesión no borra su gasto, y por eso tampoco devuelve la
//!   cuota (la cuota cuenta lo iniciado, no lo que quedó).
//!
//! Los topes por plan no viven acá: están en `plans.monthly_simulations` (null = sin tope).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE TABLE usage_ledger (
    id                   uuid PRIMARY KEY,
    user_id              uuid REFERENCES users (id) ON DELETE SET NULL,
    interview_session_id uuid REFERENCES interview_sessions (id) ON DELETE SET NULL,
    provider             varchar NOT NULL,
    operation            varchar NOT NULL,
    units                integer NOT NULL CHECK (units >= 0),
    cost_micros          bigint NOT NULL DEFAULT 0 CHECK (cost_micros >= 0),
    occurred_at          timestamptz NOT NULL DEFAULT now()
);
-- Cuenta del mes de una persona: (user_id, operation) y el rango de fechas.
CREATE INDEX usage_ledger_user_operation_idx ON usage_ledger (user_id, operation, occurred_at);
CREATE INDEX usage_ledger_session_idx ON usage_ledger (interview_session_id);
"#;

const DOWN: &str = r#"
DROP TABLE IF EXISTS usage_ledger;
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(DOWN).await?;
        Ok(())
    }
}

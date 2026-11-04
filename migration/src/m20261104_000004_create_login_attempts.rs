use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// Contador de intentos fallidos de ingreso, por email (su HMAC, nunca el email).
// No tiene FK a `users`: también cuenta emails que no existen, para que el bloqueo no revele
// cuáles tienen cuenta.
const UP: &str = r#"
CREATE TABLE login_attempts (
    email_hash        bytea PRIMARY KEY,
    failures          integer NOT NULL,
    window_started_at timestamptz NOT NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    created_by        uuid,
    updated_at        timestamptz NOT NULL DEFAULT now(),
    updated_by        uuid
);
"#;

const DOWN: &str = "DROP TABLE IF EXISTS login_attempts;";

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

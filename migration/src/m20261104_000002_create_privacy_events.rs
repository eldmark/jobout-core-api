//! `privacy_events`: constancia de borrado. Sin FK a `users` a propósito: si la tuviera,
//! borrar al usuario borraría su propia constancia. Guarda el HMAC del email, nunca el id.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE TYPE privacy_event_kind AS ENUM ('account_deleted');

CREATE TABLE privacy_events (
    id           uuid PRIMARY KEY,
    subject_hash bytea NOT NULL,
    kind         privacy_event_kind NOT NULL,
    notes        varchar,
    created_at   timestamptz NOT NULL DEFAULT now(),
    created_by   uuid,
    updated_at   timestamptz NOT NULL DEFAULT now(),
    updated_by   uuid
);
CREATE INDEX privacy_events_subject_hash_idx ON privacy_events (subject_hash);
"#;

const DOWN: &str = r#"
DROP TABLE IF EXISTS privacy_events;
DROP TYPE IF EXISTS privacy_event_kind;
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

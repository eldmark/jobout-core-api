//! CV del candidato: archivos (con versiones y uno primario), lo extraído y la bandeja de
//! borrados pendientes del almacén. `storage_key` va cifrada (bytea).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = "
CREATE TYPE cv_extraction_status AS ENUM ('ready', 'failed');

CREATE TABLE cvs (
    id                uuid PRIMARY KEY,
    applicant_id      uuid NOT NULL REFERENCES applicants (id) ON DELETE CASCADE,
    storage_key       bytea NOT NULL,
    original_filename varchar NOT NULL,
    mime_type         varchar NOT NULL,
    size_bytes        bigint NOT NULL CHECK (size_bytes >= 0),
    version           integer NOT NULL CHECK (version >= 1),
    is_primary        boolean NOT NULL DEFAULT false,
    created_at        timestamptz NOT NULL DEFAULT now(),
    created_by        uuid,
    updated_at        timestamptz NOT NULL DEFAULT now(),
    updated_by        uuid,
    UNIQUE (applicant_id, version)
);
CREATE UNIQUE INDEX cvs_one_primary_per_applicant_idx ON cvs (applicant_id) WHERE is_primary;

CREATE TABLE cv_extractions (
    id                    uuid PRIMARY KEY,
    cv_id                 uuid NOT NULL UNIQUE REFERENCES cvs (id) ON DELETE CASCADE,
    extracted             jsonb NOT NULL,
    status                cv_extraction_status NOT NULL,
    reason                varchar,
    model                 varchar NOT NULL,
    extracted_at          timestamptz NOT NULL DEFAULT now(),
    reviewed_by_applicant boolean NOT NULL DEFAULT false,
    created_at            timestamptz NOT NULL DEFAULT now(),
    created_by            uuid,
    updated_at            timestamptz NOT NULL DEFAULT now(),
    updated_by            uuid
);

-- Bandeja de salida del borrado de archivos. Sin FK a `users`: tiene que sobrevivir al
-- borrado de la cuenta (misma razón que `privacy_events`).
CREATE TABLE pending_storage_deletions (
    id              uuid PRIMARY KEY,
    storage_key     bytea NOT NULL,
    attempts        integer NOT NULL DEFAULT 0,
    last_attempt_at timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now(),
    created_by      uuid,
    updated_at      timestamptz NOT NULL DEFAULT now(),
    updated_by      uuid
);
";

const DOWN: &str = "
DROP TABLE IF EXISTS pending_storage_deletions;
DROP TABLE IF EXISTS cv_extractions;
DROP TABLE IF EXISTS cvs;
DROP TYPE IF EXISTS cv_extraction_status;
";

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

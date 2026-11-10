//! Empresas, membresía de empresa, reclutadores, invitaciones, auditoría y contador de intentos
//! (Fase 5a, paso 3). Contrato en `documentation/api.md`, «Empresas, reclutadores e
//! invitaciones»; tablas en `documentation/db.md`.
//!
//! - `companies.tax_id` va **cifrado** (`bytea`) y `tax_id_hash` (HMAC) es único: es lo que
//!   impide dos empresas con el mismo NIT sin poder leerlo desde la base. `verified_at` es nulo
//!   hasta que la plataforma verifica a mano.
//! - `company_recruiters` une persona y empresa con su `role`. Un **índice único parcial** sobre
//!   `user_id` donde `deleted_at` es nulo hace que una persona pertenezca a una sola empresa.
//! - `company_invitations` guarda el SHA-256 del token y el HMAC del correo; nunca ninguno de
//!   los dos en claro.
//! - `access_audit` no lleva contenido, y la IP va hasheada.
//! - `rate_events` es el contador de intentos (crear empresas, aceptar invitaciones): una fila
//!   por intento, para que el límite sobreviva al proceso.
//!
//! No toca `job_offers`: la hace otro paso.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE TYPE membership_status AS ENUM ('active', 'expired', 'cancelled');

CREATE TABLE companies (
    id                   uuid PRIMARY KEY,
    name                 varchar NOT NULL,
    tax_id               bytea NOT NULL,
    tax_id_hash          bytea NOT NULL,
    website              varchar,
    phone                varchar,
    description          text,
    active               boolean NOT NULL DEFAULT true,
    verified_at          timestamptz,
    deleted_at           timestamptz,
    country_id           uuid NOT NULL REFERENCES countries (id),
    employment_sector_id uuid NOT NULL REFERENCES employment_sectors (id),
    created_at           timestamptz NOT NULL DEFAULT now(),
    created_by           uuid REFERENCES users (id) ON DELETE SET NULL,
    updated_at           timestamptz NOT NULL DEFAULT now(),
    updated_by           uuid,
    CONSTRAINT companies_tax_id_hash_key UNIQUE (tax_id_hash)
);

CREATE TABLE memberships (
    id                 uuid PRIMARY KEY,
    company_id         uuid NOT NULL REFERENCES companies (id) ON DELETE CASCADE,
    membership_type_id uuid NOT NULL REFERENCES membership_types (id),
    start_date         date NOT NULL,
    expiration_date    date NOT NULL,
    status             membership_status NOT NULL DEFAULT 'active',
    created_at         timestamptz NOT NULL DEFAULT now(),
    created_by         uuid,
    updated_at         timestamptz NOT NULL DEFAULT now(),
    updated_by         uuid,
    CONSTRAINT memberships_dates_check CHECK (expiration_date >= start_date)
);
CREATE UNIQUE INDEX memberships_one_active_per_company ON memberships (company_id) WHERE status = 'active';

CREATE TABLE company_recruiters (
    id         uuid PRIMARY KEY,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    company_id uuid NOT NULL REFERENCES companies (id),
    role       varchar NOT NULL,
    position   varchar,
    deleted_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid,
    CONSTRAINT company_recruiters_role_check CHECK (role IN ('company_admin', 'recruiter'))
);
CREATE UNIQUE INDEX company_recruiters_one_company_per_user
    ON company_recruiters (user_id) WHERE deleted_at IS NULL;
CREATE INDEX company_recruiters_company_idx ON company_recruiters (company_id, created_at, id);

CREATE TABLE company_invitations (
    id          uuid PRIMARY KEY,
    company_id  uuid NOT NULL REFERENCES companies (id) ON DELETE CASCADE,
    email_hash  bytea NOT NULL,
    role        varchar NOT NULL,
    token_hash  bytea NOT NULL,
    expires_at  timestamptz NOT NULL,
    used_at     timestamptz,
    revoked_at  timestamptz,
    created_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT company_invitations_token_hash_key UNIQUE (token_hash),
    CONSTRAINT company_invitations_role_check CHECK (role IN ('company_admin', 'recruiter'))
);
CREATE INDEX company_invitations_company_idx ON company_invitations (company_id, created_at, id);

CREATE TABLE access_audit (
    id            uuid PRIMARY KEY,
    actor_user_id uuid REFERENCES users (id) ON DELETE SET NULL,
    action        varchar NOT NULL,
    resource_type varchar NOT NULL,
    resource_id   uuid,
    ip_hash       bytea,
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX access_audit_actor_idx ON access_audit (actor_user_id, created_at);

CREATE TABLE rate_events (
    id         uuid PRIMARY KEY,
    kind       varchar NOT NULL,
    subject_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX rate_events_window_idx ON rate_events (kind, subject_id, created_at);
"#;

const DOWN: &str = r#"
DROP TABLE IF EXISTS rate_events;
DROP TABLE IF EXISTS access_audit;
DROP TABLE IF EXISTS company_invitations;
DROP TABLE IF EXISTS company_recruiters;
DROP TABLE IF EXISTS memberships;
DROP TABLE IF EXISTS companies;
DROP TYPE IF EXISTS membership_status;
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

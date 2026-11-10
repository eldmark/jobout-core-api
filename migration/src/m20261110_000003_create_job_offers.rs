//! Ofertas de empleo (Fase 5a, paso 4): `job_offers` y sus tres tablas de requisitos
//! (`job_offer_skills`, `job_offer_languages`, `job_offer_driver_licenses`), más la FK que
//! `interview_sessions.job_offer_id` esperaba desde la Fase 3.
//!
//! Decisiones (db.md, «Ofertas»):
//!
//! - **Sin columnas de edad, género ni estado civil**, ni `json`/`array`: los requisitos son
//!   filas. `description` existe una sola vez.
//! - `company_visible` es una columna **generada** (`NOT is_confidential`): no se escribe, no
//!   sale de la API y no puede quedar inconsistente con `is_confidential`.
//! - `created_by` es **nullable** y `ON DELETE SET NULL`: borrar al reclutador no borra la oferta
//!   de la empresa (BO2).
//! - `closed_at` marca el cierre (a mano o automático) y es la base de la retención de 90 días;
//!   va siempre junto con `status = 'closed'`.
//! - `applications_count` es **transitorio**: `job_applications` aún no existe (paso 6). El paso 6
//!   lo sustituye por la cuenta real y borra la columna; hasta entonces cuenta cero y permite
//!   probar el congelamiento de la oferta (OF7).
//! - La FK de `interview_sessions.job_offer_id` es `ON DELETE SET NULL`: la sesión de práctica
//!   sobrevive a la oferta. Como la columna no tenía FK, cualquier valor previo es huérfano y
//!   se anula antes de crearla.
//!
//! `down` quita la FK y las tablas; la columna `job_offer_id` queda como estaba.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE TYPE job_offer_status AS ENUM ('draft', 'published', 'paused', 'closed');

CREATE TABLE job_offers (
    id                              uuid PRIMARY KEY,
    company_id                      uuid NOT NULL REFERENCES companies (id),
    created_by                      uuid REFERENCES company_recruiters (id) ON DELETE SET NULL,
    name                            varchar(120) NOT NULL,
    description                     text,
    status                          job_offer_status NOT NULL DEFAULT 'draft',
    is_experience_required          boolean NOT NULL DEFAULT false,
    required_experience             integer CHECK (required_experience BETWEEN 0 AND 50),
    main_language_id                uuid REFERENCES languages (id),
    is_confidential                 boolean NOT NULL DEFAULT false,
    company_visible                 boolean GENERATED ALWAYS AS (NOT is_confidential) STORED NOT NULL,
    featured                        boolean NOT NULL DEFAULT false,
    schedule                        varchar(120),
    required_availability_to_travel boolean NOT NULL DEFAULT false,
    required_driver_license         boolean NOT NULL DEFAULT false,
    vacancies                       integer CHECK (vacancies BETWEEN 1 AND 999),
    hiring_date                     date,
    publication_date                date,
    closed_at                       timestamptz,
    profession_id                   uuid REFERENCES professions (id),
    employment_sector_id            uuid REFERENCES employment_sectors (id),
    job_hierarchy_id                uuid REFERENCES job_hierarchies (id),
    minimum_academic_level_id       uuid REFERENCES academic_levels (id),
    state_id                        uuid REFERENCES states (id),
    city_id                         uuid REFERENCES cities (id),
    salary_range_id                 uuid REFERENCES salary_ranges (id),
    working_shift_id                uuid REFERENCES working_shifts (id),
    contract_type_id                uuid REFERENCES contract_types (id),
    applications_count              integer NOT NULL DEFAULT 0 CHECK (applications_count >= 0),
    created_at                      timestamptz NOT NULL DEFAULT now(),
    updated_at                      timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT job_offers_closed_at_matches_status
        CHECK ((status = 'closed') = (closed_at IS NOT NULL))
);
-- Listado de la empresa (más nuevas primero, con filtro por estado) y límite por hora.
CREATE INDEX job_offers_company_idx ON job_offers (company_id, created_at DESC, id DESC);
CREATE INDEX job_offers_company_status_idx ON job_offers (company_id, status, created_at DESC, id DESC);
-- Cierre automático: solo las que siguen abiertas.
CREATE INDEX job_offers_open_hiring_date_idx ON job_offers (hiring_date)
    WHERE status IN ('published', 'paused');
-- FK de created_by.
CREATE INDEX job_offers_created_by_idx ON job_offers (created_by) WHERE created_by IS NOT NULL;

CREATE TABLE job_offer_skills (
    job_offer_id uuid NOT NULL REFERENCES job_offers (id) ON DELETE CASCADE,
    skill_id     uuid NOT NULL REFERENCES skills (id),
    is_required  boolean NOT NULL DEFAULT true,
    min_level    skill_level,
    PRIMARY KEY (job_offer_id, skill_id)
);
CREATE INDEX job_offer_skills_skill_idx ON job_offer_skills (skill_id);

CREATE TABLE job_offer_languages (
    job_offer_id    uuid NOT NULL REFERENCES job_offers (id) ON DELETE CASCADE,
    language_id     uuid NOT NULL REFERENCES languages (id),
    min_proficiency proficiency NOT NULL,
    PRIMARY KEY (job_offer_id, language_id)
);
CREATE INDEX job_offer_languages_language_idx ON job_offer_languages (language_id);

CREATE TABLE job_offer_driver_licenses (
    job_offer_id      uuid NOT NULL REFERENCES job_offers (id) ON DELETE CASCADE,
    driver_license_id uuid NOT NULL REFERENCES driver_licenses (id),
    PRIMARY KEY (job_offer_id, driver_license_id)
);
CREATE INDEX job_offer_driver_licenses_license_idx ON job_offer_driver_licenses (driver_license_id);

-- La FK que esperaba a esta tabla. Antes no había contra qué validar: todo valor es huérfano.
UPDATE interview_sessions SET job_offer_id = NULL WHERE job_offer_id IS NOT NULL;
ALTER TABLE interview_sessions
    ADD CONSTRAINT interview_sessions_job_offer_fk
    FOREIGN KEY (job_offer_id) REFERENCES job_offers (id) ON DELETE SET NULL;
CREATE INDEX interview_sessions_job_offer_idx ON interview_sessions (job_offer_id)
    WHERE job_offer_id IS NOT NULL;
"#;

const DOWN: &str = r#"
DROP INDEX IF EXISTS interview_sessions_job_offer_idx;
ALTER TABLE interview_sessions DROP CONSTRAINT IF EXISTS interview_sessions_job_offer_fk;
DROP TABLE IF EXISTS job_offer_driver_licenses;
DROP TABLE IF EXISTS job_offer_languages;
DROP TABLE IF EXISTS job_offer_skills;
DROP TABLE IF EXISTS job_offers;
DROP TYPE IF EXISTS job_offer_status;
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(UP)
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DOWN)
            .await
            .map(|_| ())
    }
}

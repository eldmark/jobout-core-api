//! Aplicaciones a ofertas con consentimiento explícito (Fase 5a, paso 6): `job_applications`,
//! `job_application_events` y `external_applications` (HU-32), y la baja de la columna
//! transitoria `job_offers.applications_count`, que sustituye la cuenta real.
//!
//! Decisiones (db.md, «job_applications»):
//!
//! - **`applicant_id` es nullable y `ON DELETE SET NULL`**: borrar la cuenta no borra la
//!   aplicación; pierde a la persona y queda como «candidato eliminado» (BO1, PA10). Un trigger
//!   `BEFORE UPDATE` vacía `shared_fields` en ese mismo instante, y un `CHECK` impide que una
//!   fila sin persona conserve una lista de campos compartidos: no queda ningún dato.
//! - `shared_fields` guarda **nombres** (jsonb), nunca valores: lo que la persona aceptó
//!   compartir se puede auditar, y el panel lee el perfil vivo filtrado por esos nombres.
//! - `UNIQUE (applicant_id, job_offer_id)`: una sola aplicación por oferta, para siempre
//!   (retirar o ser rechazada no la libera). Las filas anónimas (`NULL`) no chocan.
//! - `job_application_events.changed_by` es nullable (`ON DELETE SET NULL` sobre `users`): el
//!   rastro sobrevive a quien decidió, sin su id (BO2, BO3). `note` es `bytea`, cifrada.
//! - `external_applications` es privada de la persona: `ON DELETE CASCADE` desde `applicants`;
//!   `notes` va cifrada.
//! - `job_offers.applications_count` se elimina. `down` la restituye con la cuenta real.
//!
//! La migración no agrega ningún índice sobre texto del perfil: los listados son por
//! `(applicant_id, applied_at)` y `(job_offer_id, applied_at)`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE TYPE job_application_status AS ENUM
    ('applied', 'under_review', 'shortlisted', 'rejected', 'withdrawn', 'hired');

CREATE TABLE job_applications (
    id                  uuid PRIMARY KEY,
    applicant_id        uuid REFERENCES applicants (id) ON DELETE SET NULL,
    job_offer_id        uuid NOT NULL REFERENCES job_offers (id),
    status              job_application_status NOT NULL DEFAULT 'applied',
    shared_fields       jsonb NOT NULL DEFAULT '[]'::jsonb,
    consented_at        timestamptz NOT NULL DEFAULT now(),
    company_revealed_at timestamptz,
    applied_at          timestamptz NOT NULL DEFAULT now(),
    status_changed_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT job_applications_applicant_offer_key UNIQUE (applicant_id, job_offer_id),
    CONSTRAINT job_applications_shared_fields_is_array CHECK (jsonb_typeof(shared_fields) = 'array'),
    CONSTRAINT job_applications_no_share_without_person
        CHECK (applicant_id IS NOT NULL OR shared_fields = '[]'::jsonb)
);
-- «Mis aplicaciones» y el límite de 24 horas (cuenta las filas, no la memoria del proceso).
CREATE INDEX job_applications_applicant_idx ON job_applications (applicant_id, applied_at DESC, id DESC)
    WHERE applicant_id IS NOT NULL;
-- Panel de la oferta (paso 7) y la cuenta de aplicaciones.
CREATE INDEX job_applications_offer_idx ON job_applications (job_offer_id, applied_at DESC, id DESC);

CREATE FUNCTION job_applications_anonymize() RETURNS trigger AS $$
BEGIN
    IF OLD.applicant_id IS NOT NULL AND NEW.applicant_id IS NULL THEN
        NEW.shared_fields := '[]'::jsonb;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER job_applications_anonymize_trg BEFORE UPDATE OF applicant_id ON job_applications
    FOR EACH ROW EXECUTE FUNCTION job_applications_anonymize();

CREATE TABLE job_application_events (
    id                 uuid PRIMARY KEY,
    job_application_id uuid NOT NULL REFERENCES job_applications (id) ON DELETE CASCADE,
    from_status        job_application_status,
    to_status          job_application_status NOT NULL,
    changed_by         uuid REFERENCES users (id) ON DELETE SET NULL,
    note               bytea,
    created_at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX job_application_events_application_idx
    ON job_application_events (job_application_id, created_at, id);

CREATE TABLE external_applications (
    id           uuid PRIMARY KEY,
    applicant_id uuid NOT NULL REFERENCES applicants (id) ON DELETE CASCADE,
    company_name varchar(120) NOT NULL,
    position     varchar(120) NOT NULL,
    applied_on   date NOT NULL,
    status       varchar NOT NULL,
    notes        bytea,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT external_applications_status_check
        CHECK (status IN ('applied', 'interviewing', 'offer', 'rejected', 'withdrawn', 'accepted'))
);
CREATE INDEX external_applications_applicant_idx
    ON external_applications (applicant_id, applied_on DESC, id DESC);

-- La cuenta real sustituye al contador transitorio.
ALTER TABLE job_offers DROP COLUMN applications_count;
"#;

const DOWN: &str = r#"
ALTER TABLE job_offers
    ADD COLUMN applications_count integer NOT NULL DEFAULT 0 CHECK (applications_count >= 0);
UPDATE job_offers o SET applications_count =
    (SELECT count(*) FROM job_applications a WHERE a.job_offer_id = o.id);
DROP TABLE IF EXISTS external_applications;
DROP TABLE IF EXISTS job_application_events;
DROP TRIGGER IF EXISTS job_applications_anonymize_trg ON job_applications;
DROP TABLE IF EXISTS job_applications;
DROP FUNCTION IF EXISTS job_applications_anonymize();
DROP TYPE IF EXISTS job_application_status;
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

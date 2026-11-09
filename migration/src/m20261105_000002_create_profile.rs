//! Perfil del candidato: datos profesionales, idiomas, skills, conocimientos extra,
//! experiencias y el teléfono (`personal_data`). Todo cuelga de `applicants` con cascada.
//!
//! Cifrado en la aplicación (bytea): `personal_data.phone` y `applicant_experiences.description`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const AUDIT: &str = "created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid";

fn up_sql() -> String {
    format!(
        "
CREATE TYPE proficiency AS ENUM ('native', 'advanced', 'intermediate', 'basic');
CREATE TYPE skill_level AS ENUM ('beginner', 'intermediate', 'advanced', 'expert');

CREATE TABLE professional_data (
    id                     uuid PRIMARY KEY,
    applicant_id           uuid NOT NULL UNIQUE REFERENCES applicants (id) ON DELETE CASCADE,
    description            text,
    last_academic_level_id uuid REFERENCES academic_levels (id),
    profession_id          uuid REFERENCES professions (id),
    last_job_hierarchy_id  uuid REFERENCES job_hierarchies (id),
    {AUDIT}
);

CREATE TABLE applicant_languages (
    id           uuid PRIMARY KEY,
    applicant_id uuid NOT NULL REFERENCES applicants (id) ON DELETE CASCADE,
    language_id  uuid NOT NULL REFERENCES languages (id),
    proficiency  proficiency NOT NULL,
    {AUDIT},
    UNIQUE (applicant_id, language_id)
);

CREATE TABLE applicant_skills (
    id                  uuid PRIMARY KEY,
    applicant_id        uuid NOT NULL REFERENCES applicants (id) ON DELETE CASCADE,
    skill_id            uuid NOT NULL REFERENCES skills (id),
    level               skill_level,
    years_of_experience integer CHECK (years_of_experience IS NULL OR years_of_experience >= 0),
    {AUDIT},
    UNIQUE (applicant_id, skill_id)
);
CREATE INDEX applicant_skills_skill_id_idx ON applicant_skills (skill_id);

CREATE TABLE applicant_extra_knowledge (
    id           uuid PRIMARY KEY,
    applicant_id uuid NOT NULL UNIQUE REFERENCES applicants (id) ON DELETE CASCADE,
    description  text NOT NULL,
    {AUDIT}
);

CREATE TABLE applicant_experiences (
    id           uuid PRIMARY KEY,
    applicant_id uuid NOT NULL REFERENCES applicants (id) ON DELETE CASCADE,
    company      varchar NOT NULL,
    title        varchar NOT NULL,
    start_date   date NOT NULL,
    end_date     date,
    description  bytea,
    position     integer NOT NULL DEFAULT 0,
    {AUDIT},
    CONSTRAINT applicant_experiences_dates_check CHECK (end_date IS NULL OR end_date >= start_date)
);
CREATE INDEX applicant_experiences_applicant_id_idx ON applicant_experiences (applicant_id);

CREATE TABLE personal_data (
    id           uuid PRIMARY KEY,
    applicant_id uuid NOT NULL UNIQUE REFERENCES applicants (id) ON DELETE CASCADE,
    phone        bytea,
    {AUDIT}
);
"
    )
}

const DOWN: &str = "
DROP TABLE IF EXISTS personal_data;
DROP TABLE IF EXISTS applicant_experiences;
DROP TABLE IF EXISTS applicant_extra_knowledge;
DROP TABLE IF EXISTS applicant_skills;
DROP TABLE IF EXISTS applicant_languages;
DROP TABLE IF EXISTS professional_data;
DROP TYPE IF EXISTS skill_level;
DROP TYPE IF EXISTS proficiency;
";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(&up_sql())
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(DOWN).await?;
        Ok(())
    }
}

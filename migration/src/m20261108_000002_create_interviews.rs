//! Sesiones de entrevista (Fase 3, paso 4): interview_sessions, interview_turns,
//! interview_scores y applicant_level_progress. Forma en `documentation/db.md`, sección
//! «Entrevista». Todo cuelga de `applicants` con cascada (borrar la cuenta borra las sesiones).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const SCHEMA: &str = "
CREATE TYPE interview_status AS ENUM ('created', 'in_progress', 'paused', 'completed', 'abandoned');
CREATE TYPE interview_speaker AS ENUM ('recruiter', 'applicant');

CREATE TABLE interview_sessions (
    id                 uuid PRIMARY KEY,
    applicant_id       uuid NOT NULL REFERENCES applicants (id) ON DELETE CASCADE,
    ai_recruiter_id    uuid NOT NULL REFERENCES ai_recruiters (id),
    interview_level_id uuid NOT NULL REFERENCES interview_levels (id),
    -- Sin FK todavía: `job_offers` no existe. La FK entra con la migración de ofertas.
    job_offer_id       uuid,
    -- Se fija al empezar, no al terminar: el score se explica con la rúbrica que lo produjo.
    rubric_version_id  uuid NOT NULL REFERENCES rubric_versions (id),
    status             interview_status NOT NULL DEFAULT 'created',
    started_at         timestamptz,
    paused_at          timestamptz,
    completed_at       timestamptz,
    last_activity_at   timestamptz NOT NULL DEFAULT now(),
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);
-- Una persona tiene a lo sumo una entrevista abierta (en curso o pausada) a la vez.
CREATE UNIQUE INDEX interview_sessions_one_open_per_applicant_idx
    ON interview_sessions (applicant_id) WHERE status IN ('in_progress', 'paused');
CREATE INDEX interview_sessions_applicant_idx ON interview_sessions (applicant_id, created_at DESC);

CREATE TABLE interview_turns (
    id                   uuid PRIMARY KEY,
    interview_session_id uuid NOT NULL REFERENCES interview_sessions (id) ON DELETE CASCADE,
    seq                  integer NOT NULL CHECK (seq >= 1),
    speaker              interview_speaker NOT NULL,
    kind                 varchar NOT NULL
                         CHECK (kind IN ('question', 'follow_up', 'answer', 'closing')),
    -- AES-256-GCM a nivel de aplicación (nonce ‖ cifrado ‖ etiqueta). Nunca texto en claro.
    transcript           bytea NOT NULL,
    audio_ms             integer CHECK (audio_ms >= 0),
    created_at           timestamptz NOT NULL DEFAULT now(),
    UNIQUE (interview_session_id, seq)
);

CREATE TABLE interview_scores (
    id                   uuid PRIMARY KEY,
    interview_session_id uuid NOT NULL UNIQUE REFERENCES interview_sessions (id) ON DELETE CASCADE,
    score                smallint NOT NULL CHECK (score BETWEEN 0 AND 100),
    -- Señales de voz: en modo texto no existe, por eso admite NULL.
    confidence           smallint CHECK (confidence BETWEEN 0 AND 100),
    clarity              smallint NOT NULL CHECK (clarity BETWEEN 0 AND 100),
    -- El «por qué» del score: no es opcional.
    breakdown            jsonb NOT NULL,
    rubric_version_id    uuid NOT NULL REFERENCES rubric_versions (id),
    computed_at          timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE applicant_level_progress (
    id                 uuid PRIMARY KEY,
    applicant_id       uuid NOT NULL REFERENCES applicants (id) ON DELETE CASCADE,
    interview_level_id uuid NOT NULL REFERENCES interview_levels (id),
    unlocked_at        timestamptz NOT NULL DEFAULT now(),
    best_score         smallint CHECK (best_score BETWEEN 0 AND 100),
    sessions_completed integer NOT NULL DEFAULT 0 CHECK (sessions_completed >= 0),
    UNIQUE (applicant_id, interview_level_id)
);
";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(SCHEMA)
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS applicant_level_progress;
                 DROP TABLE IF EXISTS interview_scores;
                 DROP TABLE IF EXISTS interview_turns;
                 DROP TABLE IF EXISTS interview_sessions;
                 DROP TYPE IF EXISTS interview_speaker;
                 DROP TYPE IF EXISTS interview_status;",
            )
            .await
            .map(|_| ())
    }
}

//! Catálogo de la entrevista (Fase 3, paso 1): ai_recruiters, rubrics, rubric_versions e
//! interview_levels, con sus semillas. Forma en `documentation/db.md`, sección «Entrevista».

use sea_orm_migration::prelude::*;

use crate::interview_seeds::{criteria_json, LEVELS, RECRUITERS};
use crate::new_id;

#[derive(DeriveMigrationName)]
pub struct Migration;

const SCHEMA: &str = "
CREATE TABLE rubrics (
    id          uuid PRIMARY KEY,
    name        varchar NOT NULL UNIQUE,
    description text,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

-- Un score de hace tres meses tiene que seguir explicándose con la rúbrica que lo produjo.
CREATE TABLE rubric_versions (
    id           uuid PRIMARY KEY,
    rubric_id    uuid NOT NULL REFERENCES rubrics (id),
    version      integer NOT NULL CHECK (version >= 1),
    criteria     jsonb NOT NULL CHECK (jsonb_typeof(criteria) = 'array'),
    published_at timestamptz,
    is_current   boolean NOT NULL DEFAULT false,
    created_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (rubric_id, version)
);
CREATE UNIQUE INDEX rubric_versions_one_current_per_rubric_idx
    ON rubric_versions (rubric_id) WHERE is_current;

-- Una versión publicada nunca se edita ni se borra: se publica otra. Lo único que cambia es
-- `is_current`, que pasa a false cuando entra la nueva.
CREATE FUNCTION rubric_versions_immutable() RETURNS trigger AS $$
BEGIN
    IF OLD.published_at IS NULL THEN
        RETURN CASE TG_OP WHEN 'DELETE' THEN OLD ELSE NEW END;
    END IF;
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'rubric_versions: una versión publicada es inmutable (no se borra)';
    END IF;
    IF (NEW.id, NEW.rubric_id, NEW.version, NEW.criteria, NEW.published_at, NEW.created_at)
       IS DISTINCT FROM
       (OLD.id, OLD.rubric_id, OLD.version, OLD.criteria, OLD.published_at, OLD.created_at) THEN
        RAISE EXCEPTION 'rubric_versions: una versión publicada es inmutable (publica otra)';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER rubric_versions_immutable_trg
    BEFORE UPDATE OR DELETE ON rubric_versions
    FOR EACH ROW EXECUTE FUNCTION rubric_versions_immutable();

CREATE TABLE ai_recruiters (
    id               uuid PRIMARY KEY,
    code             varchar NOT NULL UNIQUE,
    display_name     varchar NOT NULL,
    role_title       varchar NOT NULL,
    personality      text NOT NULL,
    system_prompt    text NOT NULL DEFAULT '',
    voice_id         varchar,
    avatar_asset_key varchar,
    language_id      uuid NOT NULL REFERENCES languages (id),
    rubric_id        uuid NOT NULL REFERENCES rubrics (id),
    active           boolean NOT NULL DEFAULT true,
    sort_order       integer NOT NULL DEFAULT 0,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE interview_levels (
    id                  uuid PRIMARY KEY,
    code                varchar NOT NULL UNIQUE,
    display_name        varchar NOT NULL,
    description         text,
    sort_order          integer NOT NULL,
    previous_level_id   uuid REFERENCES interview_levels (id),
    min_score_to_unlock smallint CHECK (min_score_to_unlock BETWEEN 1 AND 100),
    available           boolean NOT NULL DEFAULT false,
    active              boolean NOT NULL DEFAULT true,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    CHECK (previous_level_id IS DISTINCT FROM id),
    -- El score mínimo solo tiene sentido si hay un nivel previo.
    CHECK ((previous_level_id IS NULL) = (min_score_to_unlock IS NULL))
);
";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(SCHEMA).await?;

        for (i, l) in LEVELS.iter().enumerate() {
            let previo = if i == 0 {
                "NULL".to_string()
            } else {
                format!(
                    "(SELECT id FROM interview_levels WHERE code = '{}')",
                    LEVELS[i - 1].code
                )
            };
            let min = l
                .min_score_to_unlock
                .map_or("NULL".to_string(), |m| m.to_string());
            db.execute_unprepared(&format!(
                "INSERT INTO interview_levels \
                 (id, code, display_name, description, sort_order, previous_level_id, min_score_to_unlock, available) \
                 VALUES ('{}', '{}', '{}', '{}', {i}, {previo}, {min}, {})",
                new_id(),
                l.code,
                l.display_name,
                l.description,
                l.available
            ))
            .await?;
        }

        for (i, r) in RECRUITERS.iter().enumerate() {
            let (rubric, version, code) = (new_id(), new_id(), r.code.as_str());
            db.execute_unprepared(&format!(
                "INSERT INTO rubrics (id, name, description) \
                 VALUES ('{rubric}', 'rubrica-{code}', 'Rúbrica de {}')",
                r.display_name
            ))
            .await?;
            db.execute_unprepared(&format!(
                "INSERT INTO rubric_versions (id, rubric_id, version, criteria, published_at, is_current) \
                 VALUES ('{version}', '{rubric}', 1, '{}'::jsonb, now(), true)",
                criteria_json(&r.criteria)
            ))
            .await?;
            db.execute_unprepared(&format!(
                "INSERT INTO ai_recruiters \
                 (id, code, display_name, role_title, personality, language_id, rubric_id, sort_order) \
                 VALUES ('{}', '{code}', '{}', '{}', '{}', \
                         (SELECT id FROM languages WHERE code = '{}'), '{rubric}', {i})",
                new_id(),
                r.display_name,
                r.role_title,
                r.personality,
                r.language
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS ai_recruiters;
                 DROP TABLE IF EXISTS rubric_versions;
                 DROP FUNCTION IF EXISTS rubric_versions_immutable();
                 DROP TABLE IF EXISTS rubrics;
                 DROP TABLE IF EXISTS interview_levels;",
            )
            .await
            .map(|_| ())
    }
}

//! Catálogos de la fase 2: languages, skills, academic_levels, professions, job_hierarchies.
//! Forma estándar de db.md: `code` único para referirse a un valor sin depender de un id.

use sea_orm_migration::prelude::*;

use crate::{new_id, seeds};

#[derive(DeriveMigrationName)]
pub struct Migration;

const CATALOGOS: [&str; 5] = [
    "languages",
    "skills",
    "academic_levels",
    "professions",
    "job_hierarchies",
];

fn create(tabla: &str, extra: &str) -> String {
    format!(
        "CREATE TABLE {tabla} (
            id          uuid PRIMARY KEY,
            code        varchar NOT NULL UNIQUE,
            name        varchar NOT NULL,
            description text,
            active      boolean NOT NULL DEFAULT true,
            sort_order  integer NOT NULL DEFAULT 0,
            {extra}
            created_at  timestamptz NOT NULL DEFAULT now(),
            created_by  uuid,
            updated_at  timestamptz NOT NULL DEFAULT now(),
            updated_by  uuid
        );"
    )
}

fn q(s: &str) -> String {
    s.replace('\'', "''")
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for tabla in CATALOGOS {
            let extra = if tabla == "skills" {
                "aliases     text[] NOT NULL DEFAULT '{}',"
            } else {
                ""
            };
            db.execute_unprepared(&create(tabla, extra)).await?;
        }
        // `sort_order` sigue el orden de la lista (los niveles y jerarquías van de menor a mayor).
        // Las skills y las profesiones se ordenan por nombre, así que quedan en 0.
        for (code, name) in seeds::LANGUAGES {
            insert(db, "languages", code, name, 0).await?;
        }
        for (i, (code, name)) in seeds::ACADEMIC_LEVELS.iter().enumerate() {
            insert(db, "academic_levels", code, name, i as i32).await?;
        }
        for (code, name) in seeds::PROFESSIONS {
            insert(db, "professions", code, name, 0).await?;
        }
        for (i, (code, name)) in seeds::JOB_HIERARCHIES.iter().enumerate() {
            insert(db, "job_hierarchies", code, name, i as i32).await?;
        }
        for (code, name, aliases) in seeds::SKILLS {
            let aliases = aliases
                .iter()
                .map(|a| format!("'{}'", q(a)))
                .collect::<Vec<_>>()
                .join(", ");
            db.execute_unprepared(&format!(
                "INSERT INTO skills (id, code, name, aliases) \
                 VALUES ('{}', '{}', '{}', ARRAY[{aliases}]::text[])",
                new_id(),
                q(code),
                q(name)
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for tabla in CATALOGOS.iter().rev() {
            db.execute_unprepared(&format!("DROP TABLE IF EXISTS {tabla};"))
                .await?;
        }
        Ok(())
    }
}

async fn insert(
    db: &impl ConnectionTrait,
    tabla: &str,
    code: &str,
    name: &str,
    orden: i32,
) -> Result<(), DbErr> {
    db.execute_unprepared(&format!(
        "INSERT INTO {tabla} (id, code, name, sort_order) VALUES ('{}', '{}', '{}', {orden})",
        new_id(),
        q(code),
        q(name)
    ))
    .await
    .map(|_| ())
}

//! Favoritos del panel del reclutador (Fase 5a, paso 7, HU-54): `company_favorite_applicants`.
//!
//! Decisiones (db.md, «company_favorite_applicants»; api.md, «Panel del reclutador»):
//!
//! - **Una fila por (empresa, persona)**, no por aplicación: se marca sobre una aplicación pero
//!   vale para la persona en toda la empresa. La restricción única hace idempotente el `PUT`.
//! - **`ON DELETE CASCADE` desde `applicants`**: borrar la cuenta borra los favoritos que las
//!   empresas tenían sobre ella (BO1), en la misma transacción del borrado.
//! - **`ON DELETE CASCADE` desde `companies`**: el favorito es de la empresa.
//! - Sin quién lo marcó: no hace falta para el panel y sería un dato más que anonimizar (BO3).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE TABLE company_favorite_applicants (
    id           uuid PRIMARY KEY,
    company_id   uuid NOT NULL REFERENCES companies (id) ON DELETE CASCADE,
    applicant_id uuid NOT NULL REFERENCES applicants (id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT company_favorite_applicants_company_applicant_key UNIQUE (company_id, applicant_id)
);
-- El borrado en cascada desde `applicants` y la limpieza al retirar buscan por persona.
CREATE INDEX company_favorite_applicants_applicant_idx ON company_favorite_applicants (applicant_id);
"#;

const DOWN: &str = r#"
DROP TABLE IF EXISTS company_favorite_applicants;
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

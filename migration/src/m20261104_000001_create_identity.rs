//! Identidad y acceso: users, credenciales, refresh tokens, roles/permisos y applicants.
//! Ver jobout-docs/documentation/db.md.

use sea_orm_migration::prelude::*;

use crate::new_id;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE TYPE user_status AS ENUM ('active', 'suspended', 'pending_verification');

CREATE TABLE users (
    id                uuid PRIMARY KEY,
    clerk_user_id     varchar UNIQUE,
    email             bytea NOT NULL,
    email_hash        bytea NOT NULL UNIQUE,
    first_name        varchar NOT NULL,
    middle_name       varchar,
    last_name         varchar NOT NULL,
    last_name2        varchar,
    married_last_name varchar,
    status            user_status NOT NULL DEFAULT 'pending_verification',
    deleted_at        timestamptz,
    created_at        timestamptz NOT NULL DEFAULT now(),
    created_by        uuid,
    updated_at        timestamptz NOT NULL DEFAULT now(),
    updated_by        uuid
);

CREATE TABLE user_credentials (
    user_id       uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    password_hash varchar NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    created_by    uuid,
    updated_at    timestamptz NOT NULL DEFAULT now(),
    updated_by    uuid
);

CREATE TABLE refresh_tokens (
    id         uuid PRIMARY KEY,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash bytea NOT NULL UNIQUE,
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid
);
CREATE INDEX refresh_tokens_user_id_idx ON refresh_tokens (user_id);

CREATE TABLE roles (
    id          uuid PRIMARY KEY,
    code        varchar NOT NULL UNIQUE,
    name        varchar NOT NULL,
    description varchar,
    deleted_at  timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now(),
    created_by  uuid,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    updated_by  uuid
);

CREATE TABLE permissions (
    id          uuid PRIMARY KEY,
    code        varchar NOT NULL UNIQUE,
    name        varchar NOT NULL,
    description varchar,
    deleted_at  timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now(),
    created_by  uuid,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    updated_by  uuid
);

CREATE TABLE role_permissions (
    role_id         uuid NOT NULL REFERENCES roles (id) ON DELETE CASCADE,
    permission_id   uuid NOT NULL REFERENCES permissions (id) ON DELETE CASCADE,
    assigned_date   timestamptz NOT NULL DEFAULT now(),
    created_at      timestamptz NOT NULL DEFAULT now(),
    created_by      uuid,
    updated_at      timestamptz NOT NULL DEFAULT now(),
    updated_by      uuid,
    PRIMARY KEY (role_id, permission_id)
);

CREATE TABLE user_roles (
    user_id       uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role_id       uuid NOT NULL REFERENCES roles (id) ON DELETE CASCADE,
    assigned_date timestamptz NOT NULL DEFAULT now(),
    created_at    timestamptz NOT NULL DEFAULT now(),
    created_by    uuid,
    updated_at    timestamptz NOT NULL DEFAULT now(),
    updated_by    uuid,
    PRIMARY KEY (user_id, role_id)
);

CREATE TABLE applicants (
    id         uuid PRIMARY KEY,
    user_id    uuid NOT NULL UNIQUE REFERENCES users (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid
);
"#;

const DOWN: &str = r#"
DROP TABLE IF EXISTS applicants;
DROP TABLE IF EXISTS user_roles;
DROP TABLE IF EXISTS role_permissions;
DROP TABLE IF EXISTS permissions;
DROP TABLE IF EXISTS roles;
DROP TABLE IF EXISTS refresh_tokens;
DROP TABLE IF EXISTS user_credentials;
DROP TABLE IF EXISTS users;
DROP TYPE IF EXISTS user_status;
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(UP).await?;
        // Roles base de la fase 1. Los ids son UUID v7 generados acá.
        for (code, name) in [("applicant", "Candidato"), ("admin", "Administrador")] {
            db.execute_unprepared(&format!(
                "INSERT INTO roles (id, code, name) VALUES ('{}', '{code}', '{name}')",
                new_id()
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(DOWN).await?;
        Ok(())
    }
}

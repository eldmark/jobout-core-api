//! Planes y suscripciones de usuario. El plan Empresarial se siembra pero se asigna a
//! empresas por `memberships`, no por `user_subscriptions`.

use sea_orm_migration::prelude::*;

use crate::new_id;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &str = r#"
CREATE TYPE subscription_status AS ENUM ('active', 'expired', 'cancelled');
CREATE TYPE subscription_source AS ENUM ('free', 'paid', 'student_trial', 'mentor');

CREATE TABLE plans (
    id                  uuid PRIMARY KEY,
    code                varchar NOT NULL UNIQUE,
    name                varchar NOT NULL,
    monthly_price_gtq   numeric(10, 2) NOT NULL,
    monthly_simulations integer,
    created_at          timestamptz NOT NULL DEFAULT now(),
    created_by          uuid,
    updated_at          timestamptz NOT NULL DEFAULT now(),
    updated_by          uuid
);

CREATE TABLE user_subscriptions (
    id         uuid PRIMARY KEY,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    plan_id    uuid NOT NULL REFERENCES plans (id),
    status     subscription_status NOT NULL,
    source     subscription_source NOT NULL,
    started_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid
);
CREATE INDEX user_subscriptions_user_id_idx ON user_subscriptions (user_id);
-- Un solo registro activo por usuario; el historial (expired/cancelled) no tiene tope.
CREATE UNIQUE INDEX user_subscriptions_one_active_idx
    ON user_subscriptions (user_id) WHERE status = 'active';
"#;

const DOWN: &str = r#"
DROP TABLE IF EXISTS user_subscriptions;
DROP TABLE IF EXISTS plans;
DROP TYPE IF EXISTS subscription_source;
DROP TYPE IF EXISTS subscription_status;
"#;

/// (code, nombre, precio mensual en quetzales, simulaciones al mes). Ver negocio.md.
const PLANES: [(&str, &str, &str, &str); 4] = [
    ("free", "Gratis", "0", "1"),
    ("student", "Estudiantil", "60", "10"),
    ("standard", "Estándar", "75", "12"),
    ("business", "Empresarial", "487.50", "NULL"),
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(UP).await?;
        for (code, name, precio, sims) in PLANES {
            db.execute_unprepared(&format!(
                "INSERT INTO plans (id, code, name, monthly_price_gtq, monthly_simulations) \
                 VALUES ('{}', '{code}', '{name}', {precio}, {sims})",
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

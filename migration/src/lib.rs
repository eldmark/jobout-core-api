pub use sea_orm_migration::prelude::*;

pub mod interview_seeds;
mod m20261104_000001_create_identity;
mod m20261104_000002_create_privacy_events;
mod m20261104_000003_create_plans_and_subscriptions;
mod m20261104_000004_create_login_attempts;
mod m20261105_000001_create_catalogs;
mod m20261105_000002_create_profile;
mod m20261105_000003_create_cvs;
mod m20261108_000001_create_interview_catalog;
mod m20261108_000002_create_interviews;
mod m20261108_000003_create_usage_ledger;
mod m20261108_000004_seed_recruiter_roles;
mod m20261110_000001_create_offer_catalogs;
pub mod offer_seeds;
pub mod seeds;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        // El esquema está diseñado en jobout-docs/documentation/db.md.
        // Se escribe por bloques: identidad -> catálogos -> perfil -> empresa/ofertas -> entrevista.
        //
        // Toda migración implementa `up` y `down`, y actualiza documentation/db.md
        // en el mismo PR. El CI de jobout-docs lo verifica.
        vec![
            Box::new(m20261104_000001_create_identity::Migration),
            Box::new(m20261104_000002_create_privacy_events::Migration),
            Box::new(m20261104_000003_create_plans_and_subscriptions::Migration),
            Box::new(m20261104_000004_create_login_attempts::Migration),
            Box::new(m20261105_000001_create_catalogs::Migration),
            Box::new(m20261105_000002_create_profile::Migration),
            Box::new(m20261105_000003_create_cvs::Migration),
            Box::new(m20261108_000001_create_interview_catalog::Migration),
            Box::new(m20261108_000002_create_interviews::Migration),
            Box::new(m20261108_000003_create_usage_ledger::Migration),
            Box::new(m20261108_000004_seed_recruiter_roles::Migration),
            Box::new(m20261110_000001_create_offer_catalogs::Migration),
        ]
    }
}

/// Genera un id nuevo. Todas las PK de JobOut son UUID v7 (db.md, «Convenciones»):
/// no se pueden enumerar y ordenan por tiempo.
pub fn new_id() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// E7: los ids generados son UUID v7, y ordenan por tiempo.
    #[test]
    fn los_ids_son_uuid_v7() {
        let a = new_id();
        assert_eq!(a.get_version_num(), 7);
        assert_eq!(a.get_variant(), uuid::Variant::RFC4122);
        let b = new_id();
        assert_ne!(a, b);
    }
}

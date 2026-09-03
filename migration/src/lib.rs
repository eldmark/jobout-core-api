pub use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        // El esquema está diseñado en jobout-docs/documentation/db.md.
        // Se escribe por bloques: identidad -> catálogos -> perfil -> empresa/ofertas -> entrevista.
        //
        // Toda migración implementa `up` y `down`, y actualiza documentation/db.md
        // en el mismo PR. El CI de jobout-docs lo verifica.
        vec![]
    }
}

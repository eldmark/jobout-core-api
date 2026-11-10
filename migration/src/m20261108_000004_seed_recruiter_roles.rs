//! Roles y permisos de la Fase 5: `company_admin` y `recruiter`, los permisos de ofertas,
//! aplicaciones, favoritos y empresa, y quién tiene cuál (db.md, «Identidad y acceso»).
//!
//! Todo se busca por `code`, nunca por id: los ids de `roles` y `permissions` son UUID v7
//! generados al sembrar. No toca `applicant` ni `admin`, salvo para darle `company.verify` a
//! `admin`. `company.verify` es solo de la plataforma: ningún rol de empresa lo recibe.
//!
//! `down` quita lo sembrado aquí (asignaciones de usuarios incluidas, por `ON DELETE CASCADE`)
//! y deja los roles de la fase 1 como estaban.

use sea_orm_migration::prelude::*;

use crate::new_id;

#[derive(DeriveMigrationName)]
pub struct Migration;

const ROLES: [(&str, &str, &str); 2] = [
    (
        "company_admin",
        "Administrador de empresa",
        "Gestiona la empresa, invita reclutadores y hace todo lo del reclutador.",
    ),
    (
        "recruiter",
        "Reclutador",
        "Crea y publica ofertas, revisa aplicaciones y marca favoritos.",
    ),
];

const PERMISSIONS: [(&str, &str); 10] = [
    ("offer.create", "Crear ofertas"),
    ("offer.update", "Editar ofertas"),
    ("offer.publish", "Publicar ofertas"),
    ("offer.close", "Cerrar ofertas"),
    ("application.read", "Ver aplicaciones"),
    ("application.review", "Mover aplicaciones entre estados"),
    ("favorite.manage", "Gestionar favoritos"),
    ("company.update", "Editar la empresa"),
    ("company.invite", "Invitar reclutadores"),
    ("company.verify", "Verificar empresas (solo la plataforma)"),
];

const RECRUITER: [&str; 7] = [
    "offer.create",
    "offer.update",
    "offer.publish",
    "offer.close",
    "application.read",
    "application.review",
    "favorite.manage",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for (code, name, description) in ROLES {
            db.execute_unprepared(&format!(
                "INSERT INTO roles (id, code, name, description) \
                 VALUES ('{}', '{code}', '{name}', '{description}')",
                new_id()
            ))
            .await?;
        }
        for (code, name) in PERMISSIONS {
            db.execute_unprepared(&format!(
                "INSERT INTO permissions (id, code, name) VALUES ('{}', '{code}', '{name}')",
                new_id()
            ))
            .await?;
        }
        let mut grants: Vec<(&str, &str)> = Vec::new();
        for p in RECRUITER {
            grants.push(("recruiter", p));
            grants.push(("company_admin", p));
        }
        grants.push(("company_admin", "company.update"));
        grants.push(("company_admin", "company.invite"));
        grants.push(("admin", "company.verify"));
        for (rol, permiso) in grants {
            db.execute_unprepared(&format!(
                "INSERT INTO role_permissions (role_id, permission_id) \
                 SELECT r.id, p.id FROM roles r, permissions p \
                 WHERE r.code = '{rol}' AND p.code = '{permiso}'"
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // Las filas de role_permissions y user_roles caen por ON DELETE CASCADE. Lo único que
        // sobrevive de `admin` es su fila de rol, que no se sembró aquí.
        let codes: Vec<String> = PERMISSIONS.iter().map(|(c, _)| format!("'{c}'")).collect();
        db.execute_unprepared(&format!(
            "DELETE FROM permissions WHERE code IN ({})",
            codes.join(", ")
        ))
        .await?;
        db.execute_unprepared("DELETE FROM roles WHERE code IN ('company_admin', 'recruiter')")
            .await?;
        Ok(())
    }
}

//! Esquema de los favoritos del panel (db.md: `company_favorite_applicants`, Fase 5a, paso 7).
//! PostgreSQL real, una base por prueba. Escritas antes que la migración.

#[allow(dead_code)]
mod common;

use common::{error_de, esquema, TestDb};
use migration::{Migrator, MigratorTrait};

async fn empresa(db: &TestDb, tag: &str) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO companies (id, name, tax_id, tax_id_hash, country_id, employment_sector_id) \
         VALUES ('{id}', '{tag}', '\\x01', '\\x{}', \
                 (SELECT id FROM countries ORDER BY code LIMIT 1), \
                 (SELECT id FROM employment_sectors ORDER BY code LIMIT 1))",
        common::hex(id.as_bytes())
    ))
    .await
    .unwrap();
    id
}

async fn favorito(
    db: &TestDb,
    company: uuid::Uuid,
    applicant: uuid::Uuid,
) -> Result<(), sea_orm::DbErr> {
    db.exec(&format!(
        "INSERT INTO company_favorite_applicants (id, company_id, applicant_id) \
         VALUES (gen_random_uuid(), '{company}', '{applicant}')"
    ))
    .await
}

/// Una fila por (empresa, persona): el segundo insert choca con la restricción única.
#[tokio::test]
async fn un_favorito_por_empresa_y_persona() {
    let db = TestDb::migrated().await;
    let (_, ap) = common::nuevo_applicant(&db, "a").await;
    let c1 = empresa(&db, "uno").await;
    let c2 = empresa(&db, "dos").await;
    favorito(&db, c1, ap).await.unwrap();
    favorito(&db, c2, ap).await.unwrap();
    let e = error_de(favorito(&db, c1, ap).await);
    assert!(
        e.contains("company_favorite_applicants_company_applicant_key"),
        "{e}"
    );
    db.cleanup().await;
}

/// BO1: borrar a la persona borra los favoritos sobre ella; borrar la empresa, los suyos.
#[tokio::test]
async fn los_favoritos_caen_con_la_persona_y_con_la_empresa() {
    let db = TestDb::migrated().await;
    let (user, ap) = common::nuevo_applicant(&db, "a").await;
    let (_, ap2) = common::nuevo_applicant(&db, "b").await;
    let c1 = empresa(&db, "uno").await;
    let c2 = empresa(&db, "dos").await;
    favorito(&db, c1, ap).await.unwrap();
    favorito(&db, c1, ap2).await.unwrap();
    favorito(&db, c2, ap2).await.unwrap();
    db.exec(&format!("DELETE FROM users WHERE id = '{user}'"))
        .await
        .unwrap();
    assert_eq!(
        db.count("SELECT count(*) FROM company_favorite_applicants")
            .await,
        2
    );
    db.exec(&format!("DELETE FROM companies WHERE id = '{c1}'"))
        .await
        .unwrap();
    assert_eq!(
        db.count("SELECT count(*) FROM company_favorite_applicants")
            .await,
        1
    );
    db.cleanup().await;
}

/// `down` quita la tabla y `up` la deja igual que antes.
#[tokio::test]
async fn la_migracion_es_reversible() {
    let db = TestDb::migrated().await;
    let antes = esquema(&db).await;
    Migrator::down(&db.conn, Some(1)).await.unwrap();
    assert_eq!(
        db.count(
            "SELECT count(*) FROM information_schema.tables \
             WHERE table_name = 'company_favorite_applicants'"
        )
        .await,
        0
    );
    Migrator::up(&db.conn, None).await.unwrap();
    assert_eq!(esquema(&db).await, antes);
    db.cleanup().await;
}

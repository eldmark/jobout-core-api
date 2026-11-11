//! Esquema de las columnas de autoría del lado empresa (Fase 5a, paso 10, BO3):
//! `companies.updated_by`, `memberships.created_by`/`updated_by` y
//! `company_recruiters.created_by`/`updated_by` pasan a FK → `users` `ON DELETE SET NULL`.
//! PostgreSQL real, una base por prueba. Escritas antes que la migración.

#[allow(dead_code)]
mod common;

use common::{esquema, TestDb};
use migration::{Migrator, MigratorTrait};

const COLUMNAS: [(&str, &str); 5] = [
    ("companies", "updated_by"),
    ("memberships", "created_by"),
    ("memberships", "updated_by"),
    ("company_recruiters", "created_by"),
    ("company_recruiters", "updated_by"),
];

/// ¿La columna tiene una FK a `users` con `ON DELETE SET NULL`?
async fn fk_a_users(db: &TestDb, tabla: &str, col: &str) -> bool {
    db.count(&format!(
        "SELECT count(*) FROM pg_constraint c \
         JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = ANY (c.conkey) \
         WHERE c.contype = 'f' AND c.conrelid = '{tabla}'::regclass AND a.attname = '{col}' \
           AND c.confrelid = 'users'::regclass AND c.confdeltype = 'n'"
    ))
    .await
        == 1
}

#[tokio::test]
async fn las_columnas_de_autoria_apuntan_a_users_con_set_null() {
    let db = TestDb::migrated().await;
    for (t, c) in COLUMNAS {
        assert!(fk_a_users(&db, t, c).await, "{t}.{c} sin FK SET NULL");
    }
    db.cleanup().await;
}

/// `down` quita las cinco FK; `up` limpia los ids que ya no existen y las vuelve a crear, con el
/// mismo esquema que antes.
#[tokio::test]
async fn la_migracion_es_reversible_y_limpia_ids_colgantes() {
    let db = TestDb::migrated().await;
    let antes = esquema(&db).await;
    Migrator::down(&db.conn, Some(1)).await.unwrap();
    for (t, c) in COLUMNAS {
        assert!(!fk_a_users(&db, t, c).await, "{t}.{c} sigue con FK");
    }
    // Sin la FK, una empresa puede quedar con el id de una cuenta que ya no existe.
    let (user, _) = common::nuevo_applicant(&db, "a").await;
    let company = uuid::Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO companies (id, name, tax_id, tax_id_hash, country_id, employment_sector_id, updated_by) \
         VALUES ('{company}', 'E', '\\x01', '\\x{}', \
                 (SELECT id FROM countries ORDER BY code LIMIT 1), \
                 (SELECT id FROM employment_sectors ORDER BY code LIMIT 1), '{user}')",
        common::hex(company.as_bytes())
    ))
    .await
    .unwrap();
    db.exec(&format!("DELETE FROM users WHERE id = '{user}'"))
        .await
        .unwrap();
    assert_eq!(
        db.count("SELECT count(*) FROM companies WHERE updated_by IS NOT NULL")
            .await,
        1
    );
    Migrator::up(&db.conn, None).await.unwrap();
    assert_eq!(
        db.count("SELECT count(*) FROM companies WHERE updated_by IS NOT NULL")
            .await,
        0
    );
    assert_eq!(esquema(&db).await, antes);
    db.cleanup().await;
}

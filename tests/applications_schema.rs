//! Esquema de las aplicaciones (db.md: `job_applications`, `job_application_events`,
//! `external_applications`) y la migración que reemplaza `job_offers.applications_count`.
//! PostgreSQL real, una base por prueba. Escritas antes que la migración.

#[allow(dead_code)]
mod common;

use common::{error_de, esquema, TestDb};
use migration::{Migrator, MigratorTrait};
use sea_orm::ConnectionTrait;

async fn nullable(db: &TestDb, tabla: &str, col: &str) -> Option<String> {
    let r = db
        .conn
        .query_one(sea_orm::Statement::from_string(
            db.conn.get_database_backend(),
            format!("SELECT is_nullable FROM information_schema.columns WHERE table_schema = 'public' AND table_name = '{tabla}' AND column_name = '{col}'"),
        ))
        .await
        .unwrap()?;
    Some(r.try_get_by_index(0).unwrap())
}

async fn oferta(db: &TestDb) -> uuid::Uuid {
    let company = uuid::Uuid::now_v7();
    let id = uuid::Uuid::now_v7();
    let pais = common::uuid_de(db, "SELECT id FROM countries ORDER BY code LIMIT 1").await;
    let sector = common::uuid_de(
        db,
        "SELECT id FROM employment_sectors ORDER BY code LIMIT 1",
    )
    .await;
    db.exec(&format!(
        "INSERT INTO companies (id, name, tax_id, tax_id_hash, country_id, employment_sector_id) \
         VALUES ('{company}', 'E', '\\x01', '\\x{}', '{pais}', '{sector}')",
        common::hex(company.as_bytes())
    ))
    .await
    .unwrap();
    db.exec(&format!(
        "INSERT INTO job_offers (id, company_id, name) VALUES ('{id}', '{company}', 'O')"
    ))
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn las_columnas_y_su_nulabilidad() {
    let db = TestDb::migrated().await;
    for (t, c, n) in [
        ("job_applications", "applicant_id", "YES"),
        ("job_applications", "job_offer_id", "NO"),
        ("job_applications", "status", "NO"),
        ("job_applications", "shared_fields", "NO"),
        ("job_applications", "consented_at", "NO"),
        ("job_applications", "company_revealed_at", "YES"),
        ("job_applications", "applied_at", "NO"),
        ("job_applications", "status_changed_at", "NO"),
        ("job_application_events", "from_status", "YES"),
        ("job_application_events", "to_status", "NO"),
        ("job_application_events", "changed_by", "YES"),
        ("job_application_events", "note", "YES"),
        ("external_applications", "notes", "YES"),
        ("external_applications", "applicant_id", "NO"),
    ] {
        assert_eq!(nullable(&db, t, c).await.as_deref(), Some(n), "{t}.{c}");
    }
    assert_eq!(
        nullable(&db, "job_offers", "applications_count").await,
        None
    );
    db.cleanup().await;
}

#[tokio::test]
async fn una_sola_aplicacion_por_persona_y_oferta_y_estados_cerrados() {
    let db = TestDb::migrated().await;
    let (_, ap) = common::nuevo_applicant(&db, "a").await;
    let o = oferta(&db).await;
    let ins = |id: uuid::Uuid, st: &str| {
        format!("INSERT INTO job_applications (id, applicant_id, job_offer_id, status, shared_fields) VALUES ('{id}', '{ap}', '{o}', '{st}', '[]')")
    };
    db.exec(&ins(uuid::Uuid::now_v7(), "applied"))
        .await
        .unwrap();
    let e = error_de(db.exec(&ins(uuid::Uuid::now_v7(), "applied")).await);
    assert!(e.contains("job_applications"), "{e}");
    assert!(db
        .exec(&ins(uuid::Uuid::now_v7(), "selected"))
        .await
        .is_err());
    db.cleanup().await;
}

/// Borrar la cuenta: la aplicación sobrevive, sin applicant_id y con `shared_fields = []`; el
/// `changed_by` de los eventos pasa a null; los registros externos se van.
#[tokio::test]
async fn borrar_al_usuario_anonimiza_en_la_base() {
    let db = TestDb::migrated().await;
    let (user, ap) = common::nuevo_applicant(&db, "a").await;
    let o = oferta(&db).await;
    let app = uuid::Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO job_applications (id, applicant_id, job_offer_id, status, shared_fields) \
         VALUES ('{app}', '{ap}', '{o}', 'applied', '[\"display_name\",\"skills\",\"experience\",\"email\"]')"
    ))
    .await
    .unwrap();
    db.exec(&format!(
        "INSERT INTO job_application_events (id, job_application_id, from_status, to_status, changed_by) \
         VALUES (gen_random_uuid(), '{app}', NULL, 'applied', '{user}')"
    ))
    .await
    .unwrap();
    db.exec(&format!(
        "INSERT INTO external_applications (id, applicant_id, company_name, position, applied_on, status) \
         VALUES (gen_random_uuid(), '{ap}', 'X', 'Y', current_date, 'applied')"
    ))
    .await
    .unwrap();
    db.exec(&format!("DELETE FROM users WHERE id = '{user}'"))
        .await
        .unwrap();
    assert_eq!(db.count("SELECT count(*) FROM job_applications WHERE applicant_id IS NULL AND shared_fields = '[]'::jsonb").await, 1);
    assert_eq!(
        db.count("SELECT count(*) FROM job_application_events WHERE changed_by IS NULL")
            .await,
        1
    );
    assert_eq!(
        db.count("SELECT count(*) FROM external_applications").await,
        0
    );
    db.cleanup().await;
}

/// La invariante: sin persona no hay lista compartida.
#[tokio::test]
async fn sin_persona_no_hay_campos_compartidos() {
    let db = TestDb::migrated().await;
    let o = oferta(&db).await;
    let r = db
        .exec(&format!(
            "INSERT INTO job_applications (id, applicant_id, job_offer_id, status, shared_fields) \
             VALUES (gen_random_uuid(), NULL, '{o}', 'applied', '[\"email\"]')"
        ))
        .await;
    let e = error_de(r);
    assert!(
        e.contains("job_applications_no_share_without_person"),
        "{e}"
    );
    db.cleanup().await;
}

/// La migración se revierte y se vuelve a aplicar: `down` devuelve la columna transitoria con
/// la cuenta real y no deja nada de lo nuevo.
#[tokio::test]
async fn la_migracion_es_reversible() {
    let db = TestDb::migrated().await;
    let (_, ap) = common::nuevo_applicant(&db, "a").await;
    let o = oferta(&db).await;
    db.exec(&format!(
        "INSERT INTO job_applications (id, applicant_id, job_offer_id, status, shared_fields) \
         VALUES (gen_random_uuid(), '{ap}', '{o}', 'applied', '[]')"
    ))
    .await
    .unwrap();
    let antes = esquema(&db).await;
    Migrator::down(&db.conn, Some(2)).await.unwrap();
    assert_eq!(nullable(&db, "job_applications", "status").await, None);
    assert_eq!(nullable(&db, "external_applications", "notes").await, None);
    assert_eq!(
        nullable(&db, "job_offers", "applications_count")
            .await
            .as_deref(),
        Some("NO")
    );
    assert_eq!(
        db.count(&format!(
            "SELECT applications_count::bigint FROM job_offers WHERE id = '{o}'"
        ))
        .await,
        1
    );
    assert_eq!(
        db.count("SELECT count(*) FROM pg_type WHERE typname = 'job_application_status'")
            .await,
        0
    );
    Migrator::up(&db.conn, None).await.unwrap();
    assert_eq!(esquema(&db).await, antes);
    db.cleanup().await;
}

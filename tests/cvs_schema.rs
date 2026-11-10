//! Casos de verdad del esquema de CV (C1–C5 de docs/working/fase2-perfil-cv.md).
//! Corren contra PostgreSQL real; ver `tests/common/mod.rs`.

mod common;

use common::{bytes_de, contiene, error_de, esquema, hex, llaves, nuevo_applicant, TestDb};
use migration::{Migrator, MigratorTrait};
use uuid::Uuid;

const TABLAS: [&str; 3] = ["cvs", "cv_extractions", "pending_storage_deletions"];

async fn nuevo_cv(db: &TestDb, ap: Uuid, version: i32, primario: bool) -> Result<Uuid, String> {
    let id = Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO cvs (id, applicant_id, storage_key, original_filename, mime_type, \
           size_bytes, version, is_primary) \
         VALUES ('{id}', '{ap}', '\\x01'::bytea, 'cv.pdf', 'application/pdf', 1000, {version}, {primario})"
    ))
    .await
    .map(|_| id)
    .map_err(|e| e.to_string())
}

/// C1: las tablas existen y `down` seguido de `up` es reversible.
#[tokio::test]
async fn c1_las_tablas_de_cv_son_reversibles() {
    let db = TestDb::migrated().await;
    for t in TABLAS {
        let n = db
            .count(&format!(
                "SELECT count(*) FROM information_schema.tables \
                 WHERE table_schema = 'public' AND table_name = '{t}'"
            ))
            .await;
        assert_eq!(n, 1, "falta la tabla {t}");
    }
    let antes = esquema(&db).await;
    Migrator::down(&db.conn, Some(5)).await.expect("down");
    for t in TABLAS {
        let n = db
            .count(&format!(
                "SELECT count(*) FROM information_schema.tables \
                 WHERE table_schema = 'public' AND table_name = '{t}'"
            ))
            .await;
        assert_eq!(n, 0, "down debe quitar {t}");
    }
    assert_eq!(
        db.count("SELECT count(*) FROM pg_type WHERE typname = 'cv_extraction_status'")
            .await,
        0
    );
    // El perfil sigue ahí.
    assert_eq!(
        db.count(
            "SELECT count(*) FROM information_schema.tables WHERE table_name = 'personal_data'"
        )
        .await,
        1
    );
    Migrator::up(&db.conn, None).await.expect("up");
    assert_eq!(esquema(&db).await, antes);
    db.cleanup().await;
}

/// C2: dos CV primarios del mismo candidato fallan; versión repetida también.
#[tokio::test]
async fn c2_un_solo_cv_primario_por_candidato() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let (_, b) = nuevo_applicant(&db, "b").await;
    nuevo_cv(&db, a, 1, true).await.unwrap();
    let err = nuevo_cv(&db, a, 2, true).await.unwrap_err();
    assert!(err.contains("unique") || err.contains("duplicate"), "{err}");
    // No primarios, todos los que se quiera.
    nuevo_cv(&db, a, 2, false).await.unwrap();
    nuevo_cv(&db, a, 3, false).await.unwrap();
    // Otro candidato tiene su propio primario, y la misma versión 1.
    nuevo_cv(&db, b, 1, true).await.unwrap();
    // La versión no se repite para el mismo candidato.
    assert!(nuevo_cv(&db, a, 2, false).await.is_err());
    db.cleanup().await;
}

/// C3: la clave de almacenamiento se guarda cifrada.
#[tokio::test]
async fn c3_storage_key_no_esta_en_claro() {
    let db = TestDb::migrated().await;
    let k = llaves();
    let (_, a) = nuevo_applicant(&db, "a").await;
    let clave = Uuid::now_v7().to_string();
    db.exec(&format!(
        "INSERT INTO cvs (id, applicant_id, storage_key, original_filename, mime_type, size_bytes, version) \
         VALUES ('{}', '{a}', '\\x{}'::bytea, 'cv.pdf', 'application/pdf', 10, 1)",
        Uuid::now_v7(),
        hex(&k.encrypt(&clave).unwrap())
    ))
    .await
    .unwrap();
    let guardada = bytes_de(&db, "SELECT storage_key FROM cvs").await;
    assert!(!contiene(&guardada, clave.as_bytes()));
    assert_eq!(k.decrypt(&guardada).unwrap(), clave);
    // Por defecto no es primario.
    assert_eq!(
        db.count("SELECT count(*) FROM cvs WHERE is_primary").await,
        0
    );
    db.cleanup().await;
}

/// C4: `pending_storage_deletions` no tiene FK y sobrevive al borrado del usuario.
#[tokio::test]
async fn c4_la_bandeja_de_borrados_sobrevive_al_usuario() {
    let db = TestDb::migrated().await;
    let fks = db
        .count(
            "SELECT count(*) FROM pg_constraint \
             WHERE conrelid = 'pending_storage_deletions'::regclass AND contype = 'f'",
        )
        .await;
    assert_eq!(fks, 0);
    let (u, _) = nuevo_applicant(&db, "a").await;
    db.exec(&format!(
        "INSERT INTO pending_storage_deletions (id, storage_key) VALUES ('{}', '\\x0102'::bytea)",
        Uuid::now_v7()
    ))
    .await
    .unwrap();
    db.exec(&format!("DELETE FROM users WHERE id = '{u}'"))
        .await
        .unwrap();
    assert_eq!(
        db.count(
            "SELECT count(*) FROM pending_storage_deletions WHERE attempts = 0 \
                  AND last_attempt_at IS NULL"
        )
        .await,
        1
    );
    db.cleanup().await;
}

/// C5: borrar un CV borra su extracción; la extracción exige un enum válido; borrar la cuenta
/// se lleva CV y extracciones.
#[tokio::test]
async fn c5_borrar_el_cv_borra_su_extraccion() {
    let db = TestDb::migrated().await;
    let (u, a) = nuevo_applicant(&db, "a").await;
    let cv1 = nuevo_cv(&db, a, 1, true).await.unwrap();
    let cv2 = nuevo_cv(&db, a, 2, false).await.unwrap();
    let ext = |cv: Uuid, estado: &str| {
        format!(
            "INSERT INTO cv_extractions (id, cv_id, extracted, status, model) \
             VALUES ('{}', '{cv}', '{{\"skills\": []}}'::jsonb, '{estado}', 'determinista-v1')",
            Uuid::now_v7()
        )
    };
    assert!(db.exec(&ext(cv1, "pendiente")).await.is_err());
    db.exec(&ext(cv1, "ready")).await.unwrap();
    db.exec(&ext(cv2, "failed")).await.unwrap();
    assert_eq!(
        db.count("SELECT count(*) FROM cv_extractions WHERE reviewed_by_applicant")
            .await,
        0
    );
    let err = error_de(db.exec(&ext(cv1, "ready")).await);
    assert!(err.contains("unique") || err.contains("duplicate"), "{err}");

    db.exec(&format!("DELETE FROM cvs WHERE id = '{cv1}'"))
        .await
        .unwrap();
    assert_eq!(db.count("SELECT count(*) FROM cv_extractions").await, 1);
    db.exec(&format!("DELETE FROM users WHERE id = '{u}'"))
        .await
        .unwrap();
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 0);
    assert_eq!(db.count("SELECT count(*) FROM cv_extractions").await, 0);
    db.cleanup().await;
}

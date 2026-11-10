//! Casos de verdad de los catálogos (K1–K5 de docs/working/fase2-perfil-cv.md).
//! K6 es unitaria y vive en `migration/src/seeds.rs`. K7–K9 (endpoint) están en `tests/catalogs.rs`.

mod common;

use common::{esquema, TestDb};
use migration::{Migrator, MigratorTrait};

const CATALOGOS: [&str; 5] = [
    "languages",
    "skills",
    "academic_levels",
    "professions",
    "job_hierarchies",
];

async fn columna(db: &TestDb, tabla: &str, col: &str) -> Option<(String, String)> {
    let r = db
        .conn
        .query_one(sea_orm::Statement::from_string(
            db.conn.get_database_backend(),
            format!(
                "SELECT data_type, is_nullable FROM information_schema.columns \
                 WHERE table_schema = 'public' AND table_name = '{tabla}' AND column_name = '{col}'"
            ),
        ))
        .await
        .unwrap()?;
    Some((
        r.try_get_by_index(0).unwrap(),
        r.try_get_by_index(1).unwrap(),
    ))
}

use sea_orm::ConnectionTrait;

/// K1: los cinco catálogos existen con la forma estándar.
#[tokio::test]
async fn k1_los_catalogos_tienen_la_forma_estandar() {
    let db = TestDb::migrated().await;
    for t in CATALOGOS {
        for (col, tipo, nulo) in [
            ("id", "uuid", "NO"),
            ("code", "character varying", "NO"),
            ("name", "character varying", "NO"),
            ("description", "text", "YES"),
            ("active", "boolean", "NO"),
            ("sort_order", "integer", "NO"),
            ("created_at", "timestamp with time zone", "NO"),
            ("created_by", "uuid", "YES"),
            ("updated_at", "timestamp with time zone", "NO"),
            ("updated_by", "uuid", "YES"),
        ] {
            assert_eq!(
                columna(&db, t, col).await,
                Some((tipo.to_string(), nulo.to_string())),
                "{t}.{col}"
            );
        }
        // `code` único, respaldado por un índice único.
        let unicos = db
            .count(&format!(
                "SELECT count(*) FROM pg_indexes WHERE tablename = '{t}' \
                 AND indexdef LIKE 'CREATE UNIQUE INDEX%(code)%'"
            ))
            .await;
        assert_eq!(unicos, 1, "{t}.code debe ser único");
    }
    // `skills` lleva alias.
    assert_eq!(
        columna(&db, "skills", "aliases").await,
        Some(("ARRAY".to_string(), "NO".to_string()))
    );
    db.cleanup().await;
}

/// K2: bajar las migraciones nuevas y subirlas deja el mismo esquema.
#[tokio::test]
async fn k2_down_y_up_dejan_el_mismo_esquema() {
    let db = TestDb::migrated().await;
    let antes = esquema(&db).await;
    Migrator::down(&db.conn, Some(10)).await.expect("down");
    for t in CATALOGOS {
        let n = db
            .count(&format!(
                "SELECT count(*) FROM information_schema.tables \
                 WHERE table_schema = 'public' AND table_name = '{t}'"
            ))
            .await;
        assert_eq!(n, 0, "down debe quitar {t}");
    }
    // La identidad sigue ahí.
    assert_eq!(
        db.count("SELECT count(*) FROM information_schema.tables WHERE table_name = 'users'")
            .await,
        1
    );
    Migrator::up(&db.conn, None).await.expect("up");
    assert_eq!(esquema(&db).await, antes);
    db.cleanup().await;
}

/// K3: un `code` repetido dentro de un catálogo falla; en otro catálogo no.
#[tokio::test]
async fn k3_code_repetido_falla() {
    let db = TestDb::migrated().await;
    for t in CATALOGOS {
        let ins = |id: uuid::Uuid| {
            format!("INSERT INTO {t} (id, code, name) VALUES ('{id}', 'dup-prueba', 'Dup')")
        };
        db.exec(&ins(uuid::Uuid::now_v7())).await.unwrap();
        let err = common::error_de(db.exec(&ins(uuid::Uuid::now_v7())).await);
        assert!(
            err.contains("unique") || err.contains("duplicate"),
            "{t}: {err}"
        );
    }
    db.cleanup().await;
}

/// K4: al menos 80 skills y las imprescindibles, con alias.
#[tokio::test]
async fn k4_hay_skills_sembradas() {
    let db = TestDb::migrated().await;
    assert!(db.count("SELECT count(*) FROM skills").await >= 80);
    for code in [
        "javascript",
        "rust",
        "sql",
        "docker",
        "react",
        "postgresql",
        "python",
        "typescript",
        "git",
        "aws",
    ] {
        let n = db
            .count(&format!(
                "SELECT count(*) FROM skills WHERE code = '{code}'"
            ))
            .await;
        assert_eq!(n, 1, "falta la skill {code}");
    }
    for (alias, code) in [
        ("js", "javascript"),
        ("postgres", "postgresql"),
        ("ts", "typescript"),
        ("k8s", "kubernetes"),
    ] {
        let n = db
            .count(&format!(
                "SELECT count(*) FROM skills WHERE code = '{code}' AND '{alias}' = ANY(aliases)"
            ))
            .await;
        assert_eq!(n, 1, "{alias} debe ser alias de {code}");
    }
    assert_eq!(
        db.count("SELECT count(*) FROM skills WHERE NOT active")
            .await,
        0
    );
    // Ningún alias se repite entre skills ni coincide con un code ajeno.
    let repetidos = db
        .count(
            "SELECT count(*) FROM (SELECT a FROM skills, unnest(aliases) a GROUP BY a \
             HAVING count(*) > 1) x",
        )
        .await;
    assert_eq!(repetidos, 0);
    let choque = db
        .count(
            "SELECT count(*) FROM skills s, unnest(s.aliases) a \
             JOIN skills o ON o.code = a WHERE o.id <> s.id",
        )
        .await;
    assert_eq!(choque, 0);
    db.cleanup().await;
}

/// K5: idiomas ISO 639-1 y el resto de catálogos sembrados.
#[tokio::test]
async fn k5_hay_idiomas_y_demas_catalogos_sembrados() {
    let db = TestDb::migrated().await;
    for (code, name) in [("es", "Español"), ("en", "Inglés"), ("pt", "Portugués")] {
        let n = db
            .count(&format!(
                "SELECT count(*) FROM languages WHERE code = '{code}' AND name = '{name}'"
            ))
            .await;
        assert_eq!(n, 1, "falta el idioma {code}");
    }
    assert!(db.count("SELECT count(*) FROM academic_levels").await >= 6);
    assert!(db.count("SELECT count(*) FROM professions").await >= 12);
    assert!(db.count("SELECT count(*) FROM job_hierarchies").await >= 6);
    db.cleanup().await;
}

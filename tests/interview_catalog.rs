//! Casos de verdad del catálogo de la entrevista (K1–K10 de docs/working/fase3-entrevista.md).
//! K8 y K10 también tienen su prueba unitaria en `migration/src/interview_seeds.rs`; acá se
//! comprueba que lo sembrado en la base es lo mismo.
//! Corren contra PostgreSQL real; ver `tests/common/mod.rs`.

mod common;

use common::{error_de, esquema, TestDb};
use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Statement};

const TABLAS: [&str; 4] = [
    "ai_recruiters",
    "rubrics",
    "rubric_versions",
    "interview_levels",
];

async fn texto(db: &TestDb, sql: &str) -> Vec<String> {
    db.conn
        .query_all(Statement::from_string(
            db.conn.get_database_backend(),
            sql.to_string(),
        ))
        .await
        .unwrap()
        .iter()
        .map(|r| r.try_get_by_index::<String>(0).unwrap())
        .collect()
}

async fn existe(db: &TestDb, tabla: &str) -> bool {
    db.count(&format!(
        "SELECT count(*) FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = '{tabla}'"
    ))
    .await
        == 1
}

/// K1: las cuatro tablas existen y `down` seguido de `up` es reversible (también con las semillas).
#[tokio::test]
async fn k1_las_tablas_del_catalogo_son_reversibles() {
    let db = TestDb::migrated().await;
    for t in TABLAS {
        assert!(existe(&db, t).await, "falta la tabla {t}");
    }
    let antes = esquema(&db).await;
    Migrator::down(&db.conn, Some(9)).await.expect("down");
    for t in TABLAS {
        assert!(!existe(&db, t).await, "down debe quitar {t}");
    }
    assert!(existe(&db, "languages").await, "el catálogo previo sigue");
    Migrator::up(&db.conn, None).await.expect("up");
    assert_eq!(esquema(&db).await, antes);
    assert_eq!(db.count("SELECT count(*) FROM ai_recruiters").await, 5);
    db.cleanup().await;
}

/// K2: cinco recruiters con `code` único y su rol.
#[tokio::test]
async fn k2_se_siembran_los_cinco_recruiters() {
    let db = TestDb::migrated().await;
    let filas = texto(
        &db,
        "SELECT code || '|' || display_name || '|' || role_title FROM ai_recruiters ORDER BY sort_order",
    )
    .await;
    assert_eq!(
        filas,
        [
            "ana|Ana|RRHH",
            "martin|Martín|CTO",
            "laura|Laura|Recruiter Big Tech",
            "carlos|Carlos|CEO de startup",
            "sofia|Sofía|Recruiter internacional",
        ]
    );
    let dup = db
        .exec(
            "INSERT INTO ai_recruiters (id, code, display_name, role_title, personality, language_id, rubric_id) \
             SELECT gen_random_uuid(), 'ana', 'X', 'X', 'X', language_id, rubric_id FROM ai_recruiters WHERE code = 'ana'",
        )
        .await;
    assert!(error_de(dup).contains("duplicate key"), "code único");
    db.cleanup().await;
}

/// K3: Sofía en inglés, los demás en español, por el catálogo `languages`.
#[tokio::test]
async fn k3_sofia_entrevista_en_ingles_y_el_resto_en_espanol() {
    let db = TestDb::migrated().await;
    let filas = texto(
        &db,
        "SELECT r.code || '|' || l.code FROM ai_recruiters r JOIN languages l ON l.id = r.language_id \
         ORDER BY r.sort_order",
    )
    .await;
    assert_eq!(
        filas,
        ["ana|es", "martin|es", "laura|es", "carlos|es", "sofia|en"]
    );
    db.cleanup().await;
}

/// K4: cinco niveles en orden; `previous_level_id` es una cadena que empieza en Junior.
#[tokio::test]
async fn k4_los_niveles_forman_una_cadena_desde_junior() {
    let db = TestDb::migrated().await;
    let niveles = texto(&db, "SELECT code FROM interview_levels ORDER BY sort_order").await;
    assert_eq!(
        niveles,
        ["junior", "startup", "amazon", "google", "mckinsey"]
    );
    let cadena = texto(
        &db,
        "SELECT l.code || '<-' || coalesce(p.code, '') FROM interview_levels l \
         LEFT JOIN interview_levels p ON p.id = l.previous_level_id ORDER BY l.sort_order",
    )
    .await;
    assert_eq!(
        cadena,
        [
            "junior<-",
            "startup<-junior",
            "amazon<-startup",
            "google<-amazon",
            "mckinsey<-google"
        ]
    );
    // Sin ciclos: recorriendo desde cada nivel se llega a Junior sin repetir.
    let n = db
        .count(
            "WITH RECURSIVE c AS ( \
               SELECT id, previous_level_id, 1 AS d FROM interview_levels \
               UNION ALL SELECT c.id, l.previous_level_id, c.d + 1 \
               FROM c JOIN interview_levels l ON l.id = c.previous_level_id WHERE c.d < 20) \
             SELECT count(*) FROM c WHERE d >= 6",
        )
        .await;
    assert_eq!(n, 0, "hay un ciclo");
    // Un nivel no puede ser su propio previo.
    let r = db
        .exec("UPDATE interview_levels SET previous_level_id = id WHERE code = 'junior'")
        .await;
    assert!(r.is_err(), "un nivel no puede preceder a sí mismo");
    db.cleanup().await;
}

/// K5: Junior no exige score; los demás exigen entre 1 y 100 (60/70/75/80).
#[tokio::test]
async fn k5_el_score_minimo_de_cada_nivel() {
    let db = TestDb::migrated().await;
    let filas = texto(
        &db,
        "SELECT code || '=' || coalesce(min_score_to_unlock::text, 'null') \
         FROM interview_levels ORDER BY sort_order",
    )
    .await;
    assert_eq!(
        filas,
        [
            "junior=null",
            "startup=60",
            "amazon=70",
            "google=75",
            "mckinsey=80"
        ]
    );
    for malo in ["0", "101"] {
        let r = db
            .exec(&format!(
                "UPDATE interview_levels SET min_score_to_unlock = {malo} WHERE code = 'startup'"
            ))
            .await;
        assert!(r.is_err(), "min_score_to_unlock {malo} debe fallar");
    }
    db.cleanup().await;
}

/// K6: solo Junior y Startup están disponibles.
#[tokio::test]
async fn k6_solo_junior_y_startup_estan_disponibles() {
    let db = TestDb::migrated().await;
    let disp = texto(
        &db,
        "SELECT code FROM interview_levels WHERE available ORDER BY sort_order",
    )
    .await;
    assert_eq!(disp, ["junior", "startup"]);
    assert_eq!(db.count("SELECT count(*) FROM interview_levels").await, 5);
    db.cleanup().await;
}

/// K7: cada recruiter tiene su rúbrica con una versión vigente, y solo una por rúbrica.
#[tokio::test]
async fn k7_una_version_vigente_por_rubrica() {
    let db = TestDb::migrated().await;
    assert_eq!(db.count("SELECT count(*) FROM rubrics").await, 5);
    let sin = db
        .count(
            "SELECT count(*) FROM ai_recruiters r WHERE (SELECT count(*) FROM rubric_versions v \
             WHERE v.rubric_id = r.rubric_id AND v.is_current) <> 1",
        )
        .await;
    assert_eq!(sin, 0, "cada rúbrica tiene exactamente una versión vigente");
    let compartida = db
        .count("SELECT count(*) - count(DISTINCT rubric_id) FROM ai_recruiters")
        .await;
    assert_eq!(compartida, 0, "cada recruiter tiene su propia rúbrica");
    // Una segunda vigente en la misma rúbrica falla por el índice parcial.
    let r = db
        .exec(
            "INSERT INTO rubric_versions (id, rubric_id, version, criteria, published_at, is_current) \
             SELECT gen_random_uuid(), rubric_id, 2, criteria, now(), true FROM rubric_versions LIMIT 1",
        )
        .await;
    assert!(error_de(r).contains("duplicate key"), "índice parcial");
    // Una versión no vigente sí cabe, y la versión es única por rúbrica.
    db.exec(
        "INSERT INTO rubric_versions (id, rubric_id, version, criteria, published_at, is_current) \
         SELECT gen_random_uuid(), rubric_id, 2, criteria, now(), false FROM rubric_versions LIMIT 1",
    )
    .await
    .expect("una versión no vigente es válida");
    let r = db
        .exec(
            "INSERT INTO rubric_versions (id, rubric_id, version, criteria, published_at, is_current) \
             SELECT gen_random_uuid(), rubric_id, 1, criteria, now(), false FROM rubric_versions LIMIT 1",
        )
        .await;
    assert!(r.is_err(), "(rubric_id, version) es único");
    db.cleanup().await;
}

/// K8 (en la base): los criterios suman 100 y cada uno trae sus cuatro campos.
#[tokio::test]
async fn k8_los_criterios_sembrados_suman_100() {
    let db = TestDb::migrated().await;
    let malas = db
        .count(
            "SELECT count(*) FROM rubric_versions v WHERE \
             (SELECT sum((c->>'max_points')::int) FROM jsonb_array_elements(v.criteria) c) <> 100 \
             OR EXISTS (SELECT 1 FROM jsonb_array_elements(v.criteria) c WHERE \
               NOT (c ? 'code' AND c ? 'name' AND c ? 'max_points' AND c ? 'description'))",
        )
        .await;
    assert_eq!(malas, 0);
    let codes = texto(
        &db,
        "SELECT DISTINCT c->>'code' FROM rubric_versions v, jsonb_array_elements(v.criteria) c ORDER BY 1",
    )
    .await;
    assert_eq!(
        codes,
        [
            "claridad",
            "especificidad",
            "estructura",
            "profundidad",
            "relevancia"
        ]
    );
    db.cleanup().await;
}

/// K9: una versión publicada no se modifica ni se borra; solo deja de ser vigente.
#[tokio::test]
async fn k9_una_version_publicada_es_inmutable() {
    let db = TestDb::migrated().await;
    for sql in [
        "UPDATE rubric_versions SET criteria = '[]'::jsonb",
        "UPDATE rubric_versions SET version = 99",
        "UPDATE rubric_versions SET published_at = now() + interval '1 day'",
        "UPDATE rubric_versions SET rubric_id = (SELECT id FROM rubrics ORDER BY name LIMIT 1)",
        "DELETE FROM rubric_versions",
    ] {
        let msg = error_de(db.exec(sql).await);
        assert!(msg.contains("inmutable"), "{sql}: {msg}");
    }
    assert_eq!(db.count("SELECT count(*) FROM rubric_versions").await, 5);
    // Publicar otra versión: la anterior solo pierde `is_current`.
    db.exec(
        "UPDATE rubric_versions SET is_current = false \
         WHERE rubric_id = (SELECT rubric_id FROM ai_recruiters WHERE code = 'ana')",
    )
    .await
    .expect("quitar la marca de vigente está permitido");
    // Una versión sin publicar (borrador) sí se edita.
    db.exec(
        "INSERT INTO rubric_versions (id, rubric_id, version, criteria, published_at, is_current) \
         SELECT gen_random_uuid(), rubric_id, 2, criteria, NULL, false FROM rubric_versions LIMIT 1",
    )
    .await
    .unwrap();
    db.exec("UPDATE rubric_versions SET criteria = '[]'::jsonb WHERE version = 2")
        .await
        .expect("un borrador se puede editar");
    db.cleanup().await;
}

/// K10 (en la base): los `code` sembrados son los de las constantes del código.
#[tokio::test]
async fn k10_los_codes_de_la_base_son_los_del_codigo() {
    let db = TestDb::migrated().await;
    let en_base = texto(&db, "SELECT code FROM ai_recruiters ORDER BY code").await;
    let mut en_codigo: Vec<String> = migration::interview_seeds::RecruiterCode::ALL
        .iter()
        .map(|c| c.as_str().to_string())
        .collect();
    en_codigo.sort();
    assert_eq!(en_base, en_codigo);
    db.cleanup().await;
}

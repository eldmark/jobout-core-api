//! Esquema de las ofertas (OF2, OF3, OF4 y las restricciones de `job_offers` en db.md):
//! tablas, enums, llaves, restricciones y la FK de `interview_sessions.job_offer_id`.
//! PostgreSQL real, una base por prueba. Escritas antes que la migración.

#[allow(dead_code)]
mod common;

use common::{error_de, esquema, TestDb};
use migration::{Migrator, MigratorTrait};
use sea_orm::ConnectionTrait;

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

async fn tabla_existe(db: &TestDb, tabla: &str) -> bool {
    db.count(&format!(
        "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public' AND table_name = '{tabla}'"
    ))
    .await
        == 1
}

/// Una empresa, un reclutador y un catálogo mínimo, por SQL. Devuelve (company_id, recruiter_id).
async fn base(db: &TestDb) -> (uuid::Uuid, uuid::Uuid) {
    let (user, _) = common::nuevo_applicant(db, "esquema").await;
    let company = uuid::Uuid::now_v7();
    let recruiter = uuid::Uuid::now_v7();
    let pais = common::uuid_de(db, "SELECT id FROM countries ORDER BY code LIMIT 1").await;
    let sector = common::uuid_de(
        db,
        "SELECT id FROM employment_sectors ORDER BY code LIMIT 1",
    )
    .await;
    db.exec(&format!(
        "INSERT INTO companies (id, name, tax_id, tax_id_hash, country_id, employment_sector_id) \
         VALUES ('{company}', 'Esquema S.A.', '\\x01'::bytea, '\\x02'::bytea, '{pais}', '{sector}')"
    ))
    .await
    .unwrap();
    db.exec(&format!(
        "INSERT INTO company_recruiters (id, user_id, company_id, role, position) \
         VALUES ('{recruiter}', '{user}', '{company}', 'recruiter', 'x')"
    ))
    .await
    .unwrap();
    (company, recruiter)
}

async fn oferta(
    db: &TestDb,
    company: uuid::Uuid,
    extra_cols: &str,
    extra_vals: &str,
) -> Result<(), sea_orm::DbErr> {
    let id = uuid::Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO job_offers (id, company_id, name{extra_cols}) VALUES ('{id}', '{company}', 'Una oferta'{extra_vals})"
    ))
    .await
}

#[tokio::test]
async fn las_tablas_y_columnas_existen_con_su_forma() {
    let db = TestDb::migrated().await;
    for t in [
        "job_offers",
        "job_offer_skills",
        "job_offer_languages",
        "job_offer_driver_licenses",
    ] {
        assert!(tabla_existe(&db, t).await, "falta {t}");
    }
    // (columna, nulable)
    for (col, nulo) in [
        ("id", "NO"),
        ("company_id", "NO"),
        ("created_by", "YES"),
        ("name", "NO"),
        ("description", "YES"),
        ("status", "NO"),
        ("is_experience_required", "NO"),
        ("required_experience", "YES"),
        ("main_language_id", "YES"),
        ("is_confidential", "NO"),
        ("company_visible", "NO"),
        ("featured", "NO"),
        ("schedule", "YES"),
        ("required_availability_to_travel", "NO"),
        ("required_driver_license", "NO"),
        ("vacancies", "YES"),
        ("hiring_date", "YES"),
        ("publication_date", "YES"),
        ("closed_at", "YES"),
        ("profession_id", "YES"),
        ("employment_sector_id", "YES"),
        ("job_hierarchy_id", "YES"),
        ("minimum_academic_level_id", "YES"),
        ("state_id", "YES"),
        ("city_id", "YES"),
        ("salary_range_id", "YES"),
        ("working_shift_id", "YES"),
        ("contract_type_id", "YES"),
        ("created_at", "NO"),
        ("updated_at", "NO"),
    ] {
        let c = columna(&db, "job_offers", col)
            .await
            .unwrap_or_else(|| panic!("falta job_offers.{col}"));
        assert_eq!(c.1, nulo, "job_offers.{col}");
    }
    assert_eq!(
        columna(&db, "job_offers", "closed_at").await.unwrap().0,
        "timestamp with time zone"
    );
    assert_eq!(
        columna(&db, "job_offers", "hiring_date").await.unwrap().0,
        "date"
    );
    assert_eq!(
        columna(&db, "job_offers", "publication_date")
            .await
            .unwrap()
            .0,
        "date"
    );
    db.cleanup().await;
}

/// OF2: `description` una sola vez. OF3: nada de JSON ni arrays. OF4: ni edad, ni género, ni
/// estado civil, en ninguna tabla de ofertas.
#[tokio::test]
async fn of2_of3_of4_no_hay_description_doble_json_ni_columnas_discriminatorias() {
    let db = TestDb::migrated().await;
    for t in [
        "job_offers",
        "job_offer_skills",
        "job_offer_languages",
        "job_offer_driver_licenses",
    ] {
        let malas = db
            .count(&format!(
                "SELECT count(*) FROM information_schema.columns WHERE table_schema = 'public' \
                 AND table_name = '{t}' AND (data_type IN ('json', 'jsonb', 'ARRAY') \
                 OR column_name ~ '(^|_)(age|gender|marital|edad|genero|civil)($|_)')"
            ))
            .await;
        assert_eq!(malas, 0, "{t} tiene una columna prohibida");
    }
    assert_eq!(
        db.count(
            "SELECT count(*) FROM information_schema.columns WHERE table_schema = 'public' \
             AND table_name = 'job_offers' AND column_name LIKE '%description%'"
        )
        .await,
        1
    );
    // El catálogo de preferencias de género no existe.
    assert!(!tabla_existe(&db, "gender_preferences").await);
    db.cleanup().await;
}

#[tokio::test]
async fn el_estado_es_un_enum_con_cuatro_valores_y_arranca_en_draft() {
    let db = TestDb::migrated().await;
    let (company, _) = base(&db).await;
    let valores = db
        .count(
            "SELECT count(*) FROM pg_enum e JOIN pg_type t ON t.oid = e.enumtypid \
             WHERE t.typname = 'job_offer_status' AND e.enumlabel IN ('draft','published','paused','closed')",
        )
        .await;
    assert_eq!(valores, 4);
    assert_eq!(
        db.count("SELECT count(*) FROM pg_enum e JOIN pg_type t ON t.oid = e.enumtypid WHERE t.typname = 'job_offer_status'")
            .await,
        4
    );
    oferta(&db, company, "", "").await.unwrap();
    assert_eq!(
        db.count("SELECT count(*) FROM job_offers WHERE status = 'draft' AND featured = false AND is_confidential = false")
            .await,
        1
    );
    assert!(
        error_de(oferta(&db, company, ", status", ", 'archivada'").await)
            .contains("job_offer_status")
    );
    db.cleanup().await;
}

#[tokio::test]
async fn company_visible_se_deriva_de_is_confidential() {
    let db = TestDb::migrated().await;
    let (company, _) = base(&db).await;
    oferta(&db, company, ", is_confidential", ", true")
        .await
        .unwrap();
    oferta(&db, company, ", is_confidential", ", false")
        .await
        .unwrap();
    assert_eq!(
        db.count("SELECT count(*) FROM job_offers WHERE is_confidential AND NOT company_visible")
            .await,
        1
    );
    assert_eq!(
        db.count("SELECT count(*) FROM job_offers WHERE NOT is_confidential AND company_visible")
            .await,
        1
    );
    // Es generada: no se puede escribir.
    let e = error_de(oferta(&db, company, ", company_visible", ", true").await);
    assert!(
        e.contains("company_visible") || e.contains("generated"),
        "{e}"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn las_restricciones_de_rango() {
    let db = TestDb::migrated().await;
    let (company, _) = base(&db).await;
    for (col, malo, bueno) in [
        ("vacancies", "0", "1"),
        ("vacancies", "1000", "999"),
        ("required_experience", "-1", "0"),
        ("required_experience", "51", "50"),
    ] {
        assert!(
            oferta(&db, company, &format!(", {col}"), &format!(", {malo}"))
                .await
                .is_err(),
            "{col}={malo} debía fallar"
        );
        oferta(&db, company, &format!(", {col}"), &format!(", {bueno}"))
            .await
            .unwrap();
    }
    // Longitudes.
    assert!(oferta(
        &db,
        company,
        ", schedule",
        &format!(", '{}'", "x".repeat(121))
    )
    .await
    .is_err());
    assert!(db
        .exec(&format!(
            "INSERT INTO job_offers (id, company_id, name) VALUES ('{}', '{company}', '{}')",
            uuid::Uuid::now_v7(),
            "x".repeat(121)
        ))
        .await
        .is_err());
    // `closed_at` y `status = 'closed'` van juntos.
    assert!(oferta(&db, company, ", status", ", 'closed'")
        .await
        .is_err());
    assert!(oferta(&db, company, ", closed_at", ", now()")
        .await
        .is_err());
    oferta(&db, company, ", status, closed_at", ", 'closed', now()")
        .await
        .unwrap();
    db.cleanup().await;
}

#[tokio::test]
async fn las_fk_de_job_offers() {
    let db = TestDb::migrated().await;
    let (company, recruiter) = base(&db).await;
    let falso = uuid::Uuid::now_v7();
    // Empresa inexistente.
    assert!(oferta(&db, falso, "", "").await.is_err());
    // Cada FK de catálogo.
    for col in [
        "main_language_id",
        "profession_id",
        "employment_sector_id",
        "job_hierarchy_id",
        "minimum_academic_level_id",
        "state_id",
        "city_id",
        "salary_range_id",
        "working_shift_id",
        "contract_type_id",
        "created_by",
    ] {
        assert!(
            oferta(&db, company, &format!(", {col}"), &format!(", '{falso}'"))
                .await
                .is_err(),
            "{col} sin FK"
        );
    }
    // created_by es nullable y se vuelve NULL si el reclutador desaparece.
    oferta(&db, company, ", created_by", &format!(", '{recruiter}'"))
        .await
        .unwrap();
    db.exec(&format!(
        "DELETE FROM company_recruiters WHERE id = '{recruiter}'"
    ))
    .await
    .unwrap();
    assert_eq!(
        db.count("SELECT count(*) FROM job_offers WHERE created_by IS NULL")
            .await,
        1
    );
    db.cleanup().await;
}

#[tokio::test]
async fn los_requisitos_son_unicos_y_se_borran_con_la_oferta() {
    let db = TestDb::migrated().await;
    let (company, _) = base(&db).await;
    let id = uuid::Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO job_offers (id, company_id, name) VALUES ('{id}', '{company}', 'x')"
    ))
    .await
    .unwrap();
    let skill = common::uuid_de(&db, "SELECT id FROM skills ORDER BY code LIMIT 1").await;
    let idioma = common::uuid_de(&db, "SELECT id FROM languages ORDER BY code LIMIT 1").await;
    let licencia =
        common::uuid_de(&db, "SELECT id FROM driver_licenses ORDER BY code LIMIT 1").await;
    db.exec(&format!(
        "INSERT INTO job_offer_skills (job_offer_id, skill_id, is_required, min_level) VALUES ('{id}', '{skill}', true, 'expert')"
    ))
    .await
    .unwrap();
    assert!(db
        .exec(&format!(
            "INSERT INTO job_offer_skills (job_offer_id, skill_id, is_required) VALUES ('{id}', '{skill}', false)"
        ))
        .await
        .is_err());
    assert!(db
        .exec(&format!(
            "INSERT INTO job_offer_skills (job_offer_id, skill_id, is_required, min_level) VALUES ('{id}', '{idioma}', true, 'master')"
        ))
        .await
        .is_err());
    db.exec(&format!(
        "INSERT INTO job_offer_languages (job_offer_id, language_id, min_proficiency) VALUES ('{id}', '{idioma}', 'basic')"
    ))
    .await
    .unwrap();
    assert!(db
        .exec(&format!(
            "INSERT INTO job_offer_languages (job_offer_id, language_id, min_proficiency) VALUES ('{id}', '{idioma}', 'native')"
        ))
        .await
        .is_err());
    // La proficiencia es obligatoria.
    assert!(db
        .exec(&format!(
            "INSERT INTO job_offer_languages (job_offer_id, language_id) VALUES ('{id}', '{}')",
            common::uuid_de(
                &db,
                "SELECT id FROM languages ORDER BY code OFFSET 1 LIMIT 1"
            )
            .await
        ))
        .await
        .is_err());
    db.exec(&format!(
        "INSERT INTO job_offer_driver_licenses (job_offer_id, driver_license_id) VALUES ('{id}', '{licencia}')"
    ))
    .await
    .unwrap();
    assert!(db
        .exec(&format!(
            "INSERT INTO job_offer_driver_licenses (job_offer_id, driver_license_id) VALUES ('{id}', '{licencia}')"
        ))
        .await
        .is_err());
    // FK a su catálogo.
    let falso = uuid::Uuid::now_v7();
    assert!(db
        .exec(&format!(
            "INSERT INTO job_offer_driver_licenses (job_offer_id, driver_license_id) VALUES ('{id}', '{falso}')"
        ))
        .await
        .is_err());
    // Borrar la oferta borra sus requisitos.
    db.exec(&format!("DELETE FROM job_offers WHERE id = '{id}'"))
        .await
        .unwrap();
    for t in [
        "job_offer_skills",
        "job_offer_languages",
        "job_offer_driver_licenses",
    ] {
        assert_eq!(
            db.count(&format!("SELECT count(*) FROM {t}")).await,
            0,
            "{t}"
        );
    }
    db.cleanup().await;
}

/// La FK pendiente desde el paso de entrevistas: `interview_sessions.job_offer_id` apunta a
/// `job_offers` y, al borrar la oferta, la sesión sobrevive con el campo en NULL.
#[tokio::test]
async fn interview_sessions_job_offer_id_ya_tiene_fk_con_set_null() {
    let db = TestDb::migrated().await;
    let (company, _) = base(&db).await;
    let (_, applicant) = common::nuevo_applicant(&db, "entrevista").await;
    let oferta_id = uuid::Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO job_offers (id, company_id, name) VALUES ('{oferta_id}', '{company}', 'x')"
    ))
    .await
    .unwrap();
    // Sesión con una oferta inexistente: la FK la rechaza.
    let sql = crear_sesion_sql(&db, applicant, Some(uuid::Uuid::now_v7())).await;
    assert!(
        db.exec(&sql).await.is_err(),
        "la FK debía rechazar una oferta inexistente"
    );
    let sql = crear_sesion_sql(&db, applicant, Some(oferta_id)).await;
    db.exec(&sql).await.unwrap();
    db.exec(&format!("DELETE FROM job_offers WHERE id = '{oferta_id}'"))
        .await
        .unwrap();
    assert_eq!(
        db.count("SELECT count(*) FROM interview_sessions WHERE job_offer_id IS NULL")
            .await,
        1,
        "la sesión debía sobrevivir con job_offer_id en NULL"
    );
    db.cleanup().await;
}

/// El INSERT de una sesión de entrevista de `applicant`, con `oferta` (o sin ella).
async fn crear_sesion_sql(
    db: &TestDb,
    applicant: uuid::Uuid,
    oferta: Option<uuid::Uuid>,
) -> String {
    let rec = common::uuid_de(db, "SELECT id FROM ai_recruiters LIMIT 1").await;
    let nivel = common::uuid_de(db, "SELECT id FROM interview_levels LIMIT 1").await;
    let rubrica = common::uuid_de(db, "SELECT id FROM rubric_versions LIMIT 1").await;
    let oferta = oferta.map_or("NULL".to_string(), |o| format!("'{o}'"));
    format!(
        "INSERT INTO interview_sessions (id, applicant_id, ai_recruiter_id, interview_level_id, job_offer_id, rubric_version_id) \
         VALUES ('{}', '{applicant}', '{rec}', '{nivel}', {oferta}, '{rubrica}')",
        uuid::Uuid::now_v7()
    )
}

/// La migración baja y sube sin dejar rastro, y `down` conserva las sesiones de entrevista.
#[tokio::test]
async fn la_migracion_baja_y_sube() {
    let db = TestDb::migrated().await;
    let antes = esquema(&db).await;
    assert!(antes.iter().any(|l| l.starts_with("job_offers.name")));
    Migrator::down(&db.conn, Some(4)).await.expect("down");
    for t in [
        "job_offers",
        "job_offer_skills",
        "job_offer_languages",
        "job_offer_driver_licenses",
    ] {
        assert!(!tabla_existe(&db, t).await, "{t} sigue tras down");
    }
    assert_eq!(
        db.count("SELECT count(*) FROM pg_type WHERE typname = 'job_offer_status'")
            .await,
        0
    );
    // La columna de sesiones sigue (solo pierde la FK).
    assert!(columna(&db, "interview_sessions", "job_offer_id")
        .await
        .is_some());
    assert_eq!(
        db.count("SELECT count(*) FROM pg_constraint WHERE conrelid = 'interview_sessions'::regclass AND contype = 'f' AND pg_get_constraintdef(oid) LIKE '%job_offer%'")
            .await,
        0
    );
    Migrator::up(&db.conn, None).await.expect("up de nuevo");
    assert_eq!(
        antes,
        esquema(&db).await,
        "up → down → up no es idempotente"
    );
    db.cleanup().await;
}

/// Un `job_offer_id` huérfano de antes de la migración no la rompe: se anula.
#[tokio::test]
async fn migrar_con_sesiones_que_apuntan_a_ofertas_inexistentes_las_anula() {
    let db = TestDb::migrated().await;
    Migrator::down(&db.conn, Some(4)).await.unwrap();
    let (_, applicant) = common::nuevo_applicant(&db, "huerfano").await;
    let sql = crear_sesion_sql(&db, applicant, Some(uuid::Uuid::now_v7())).await;
    db.exec(&sql).await.unwrap();
    Migrator::up(&db.conn, None)
        .await
        .expect("la migración no debe fallar");
    assert_eq!(
        db.count("SELECT count(*) FROM interview_sessions WHERE job_offer_id IS NULL")
            .await,
        1
    );
    db.cleanup().await;
}

#[tokio::test]
async fn los_indices_utiles_existen() {
    let db = TestDb::migrated().await;
    let n = db
        .count(
            "SELECT count(*) FROM pg_indexes WHERE schemaname = 'public' AND tablename = 'job_offers' \
             AND indexdef LIKE '%company_id%'",
        )
        .await;
    assert!(n >= 1, "falta un índice por empresa");
    let n = db
        .count(
            "SELECT count(*) FROM pg_indexes WHERE schemaname = 'public' AND tablename = 'interview_sessions' \
             AND indexdef LIKE '%job_offer_id%'",
        )
        .await;
    assert!(n >= 1, "falta el índice de la FK de sesiones");
    db.cleanup().await;
}

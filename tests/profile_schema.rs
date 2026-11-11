//! Casos de verdad del esquema de perfil (E1–E7 de docs/working/fase2-perfil-cv.md).
//! Corren contra PostgreSQL real; ver `tests/common/mod.rs`.

mod common;

use common::{
    bytes_de, contiene, error_de, esquema, hex, llaves, nuevo_applicant, uuid_de, TestDb,
};
use migration::{Migrator, MigratorTrait};
use uuid::Uuid;

const TABLAS: [&str; 6] = [
    "professional_data",
    "applicant_languages",
    "applicant_skills",
    "applicant_extra_knowledge",
    "applicant_experiences",
    "personal_data",
];

/// E1: las tablas existen y `down` seguido de `up` es reversible.
#[tokio::test]
async fn e1_las_tablas_de_perfil_son_reversibles() {
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
    // Dos pasos: cvs y perfil.
    Migrator::down(&db.conn, Some(11)).await.expect("down");
    for t in TABLAS {
        let n = db
            .count(&format!(
                "SELECT count(*) FROM information_schema.tables \
                 WHERE table_schema = 'public' AND table_name = '{t}'"
            ))
            .await;
        assert_eq!(n, 0, "down debe quitar {t}");
    }
    let tipos = db
        .count("SELECT count(*) FROM pg_type WHERE typname IN ('proficiency','skill_level')")
        .await;
    assert_eq!(tipos, 0, "down debe quitar los enums");
    Migrator::up(&db.conn, None).await.expect("up");
    assert_eq!(esquema(&db).await, antes);
    db.cleanup().await;
}

/// E2: una skill inexistente falla por FK.
#[tokio::test]
async fn e2_skill_inexistente_falla() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let err = error_de(
        db.exec(&format!(
            "INSERT INTO applicant_skills (id, applicant_id, skill_id) \
             VALUES ('{}', '{a}', '{}')",
            Uuid::now_v7(),
            Uuid::now_v7()
        ))
        .await,
    );
    assert!(err.contains("foreign key"), "{err}");
    db.cleanup().await;
}

/// E3: la misma skill dos veces para el mismo candidato falla; para otro candidato no.
#[tokio::test]
async fn e3_la_misma_skill_dos_veces_falla() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let (_, b) = nuevo_applicant(&db, "b").await;
    let skill = uuid_de(&db, "SELECT id FROM skills WHERE code = 'rust'").await;
    let ins = |ap: Uuid, extra: &str| {
        format!(
            "INSERT INTO applicant_skills (id, applicant_id, skill_id{extra}) \
             VALUES ('{}', '{ap}', '{skill}'{})",
            Uuid::now_v7(),
            if extra.is_empty() {
                ""
            } else {
                ", 'expert', 3"
            }
        )
    };
    db.exec(&ins(a, "")).await.unwrap(); // level y años son nulos
    let err = error_de(db.exec(&ins(a, "")).await);
    assert!(err.contains("unique") || err.contains("duplicate"), "{err}");
    db.exec(&ins(b, ", level, years_of_experience"))
        .await
        .unwrap();
    // Lo mismo para idiomas.
    let idioma = uuid_de(&db, "SELECT id FROM languages WHERE code = 'en'").await;
    let il = |ap: Uuid| {
        format!(
            "INSERT INTO applicant_languages (id, applicant_id, language_id, proficiency) \
             VALUES ('{}', '{ap}', '{idioma}', 'native')",
            Uuid::now_v7()
        )
    };
    db.exec(&il(a)).await.unwrap();
    assert!(db.exec(&il(a)).await.is_err());
    db.cleanup().await;
}

/// E4: valores fuera de los enums fallan; los válidos pasan.
#[tokio::test]
async fn e4_proficiency_y_level_son_enums() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let idioma = uuid_de(&db, "SELECT id FROM languages WHERE code = 'es'").await;
    let skill = uuid_de(&db, "SELECT id FROM skills WHERE code = 'sql'").await;
    let il = |valor: &str| {
        format!(
            "INSERT INTO applicant_languages (id, applicant_id, language_id, proficiency) \
             VALUES ('{}', '{a}', '{idioma}', '{valor}')",
            Uuid::now_v7()
        )
    };
    assert!(db.exec(&il("fluent")).await.is_err());
    db.exec(&il("basic")).await.unwrap();
    for ok in ["native", "advanced", "intermediate"] {
        db.exec(&format!(
            "UPDATE applicant_languages SET proficiency = '{ok}'"
        ))
        .await
        .unwrap();
    }
    let sk = |valor: &str| {
        format!(
            "INSERT INTO applicant_skills (id, applicant_id, skill_id, level) \
             VALUES ('{}', '{a}', '{skill}', '{valor}')",
            Uuid::now_v7()
        )
    };
    assert!(db.exec(&sk("guru")).await.is_err());
    db.exec(&sk("beginner")).await.unwrap();
    for ok in ["intermediate", "advanced", "expert"] {
        db.exec(&format!("UPDATE applicant_skills SET level = '{ok}'"))
            .await
            .unwrap();
    }
    db.cleanup().await;
}

/// E5: el teléfono y la descripción de una experiencia se guardan cifrados.
#[tokio::test]
async fn e5_telefono_y_descripcion_no_estan_en_claro() {
    let db = TestDb::migrated().await;
    let k = llaves();
    let (_, a) = nuevo_applicant(&db, "a").await;
    let telefono = "+502 5555-1234";
    let descripcion = "Migré el monolito de Acme a servicios";
    db.exec(&format!(
        "INSERT INTO personal_data (id, applicant_id, phone) \
         VALUES ('{}', '{a}', '\\x{}'::bytea)",
        Uuid::now_v7(),
        hex(&k.encrypt(telefono).unwrap())
    ))
    .await
    .unwrap();
    db.exec(&format!(
        "INSERT INTO applicant_experiences \
         (id, applicant_id, company, title, start_date, description, position) \
         VALUES ('{}', '{a}', 'Acme', 'Dev', '2021-01-01', '\\x{}'::bytea, 0)",
        Uuid::now_v7(),
        hex(&k.encrypt(descripcion).unwrap())
    ))
    .await
    .unwrap();

    let tel = bytes_de(&db, "SELECT phone FROM personal_data").await;
    assert!(!contiene(&tel, telefono.as_bytes()));
    assert!(!contiene(&tel, b"5555"));
    assert_eq!(k.decrypt(&tel).unwrap(), telefono);

    let desc = bytes_de(&db, "SELECT description FROM applicant_experiences").await;
    assert!(!contiene(&desc, b"monolito"));
    assert_eq!(k.decrypt(&desc).unwrap(), descripcion);

    // `personal_data` guarda solo el teléfono: no hay otras columnas de datos personales.
    let cols = db
        .count(
            "SELECT count(*) FROM information_schema.columns WHERE table_name = 'personal_data' \
             AND column_name IN ('document_id','date_of_birth','address','mobile')",
        )
        .await;
    assert_eq!(cols, 0);
    // El teléfono es único por candidato.
    assert!(db
        .exec(&format!(
            "INSERT INTO personal_data (id, applicant_id, phone) \
             VALUES ('{}', '{a}', '\\x01'::bytea)",
            Uuid::now_v7()
        ))
        .await
        .is_err());
    db.cleanup().await;
}

/// E6: borrar el `users` borra en cascada todo el perfil.
#[tokio::test]
async fn e6_borrar_el_usuario_borra_el_perfil() {
    let db = TestDb::migrated().await;
    let (u, a) = nuevo_applicant(&db, "a").await;
    let (_, otro) = nuevo_applicant(&db, "b").await;
    for ap in [a, otro] {
        db.exec(&format!(
            "INSERT INTO professional_data (id, applicant_id, description, last_academic_level_id, \
               profession_id, last_job_hierarchy_id) \
             VALUES ('{}', '{ap}', 'Dev', \
               (SELECT id FROM academic_levels LIMIT 1), (SELECT id FROM professions LIMIT 1), \
               (SELECT id FROM job_hierarchies LIMIT 1))",
            Uuid::now_v7()
        ))
        .await
        .unwrap();
        db.exec(&format!(
            "INSERT INTO applicant_languages (id, applicant_id, language_id, proficiency) \
             VALUES ('{}', '{ap}', (SELECT id FROM languages LIMIT 1), 'basic')",
            Uuid::now_v7()
        ))
        .await
        .unwrap();
        db.exec(&format!(
            "INSERT INTO applicant_skills (id, applicant_id, skill_id) \
             VALUES ('{}', '{ap}', (SELECT id FROM skills LIMIT 1))",
            Uuid::now_v7()
        ))
        .await
        .unwrap();
        db.exec(&format!(
            "INSERT INTO applicant_extra_knowledge (id, applicant_id, description) \
             VALUES ('{}', '{ap}', 'Ajedrez')",
            Uuid::now_v7()
        ))
        .await
        .unwrap();
        db.exec(&format!(
            "INSERT INTO applicant_experiences (id, applicant_id, company, title, start_date, position) \
             VALUES ('{}', '{ap}', 'Acme', 'Dev', '2021-01-01', 0)",
            Uuid::now_v7()
        ))
        .await
        .unwrap();
        db.exec(&format!(
            "INSERT INTO personal_data (id, applicant_id, phone) \
             VALUES ('{}', '{ap}', '\\x01'::bytea)",
            Uuid::now_v7()
        ))
        .await
        .unwrap();
    }
    db.exec(&format!("DELETE FROM users WHERE id = '{u}'"))
        .await
        .unwrap();
    for t in TABLAS {
        // Solo queda la fila del otro candidato: la cascada no se llevó de más.
        assert_eq!(
            db.count(&format!("SELECT count(*) FROM {t}")).await,
            1,
            "{t}"
        );
        assert_eq!(
            db.count(&format!(
                "SELECT count(*) FROM {t} WHERE applicant_id = '{a}'"
            ))
            .await,
            0,
            "{t} conserva filas del borrado"
        );
    }
    db.cleanup().await;
}

/// E7: una experiencia que termina antes de empezar falla; terminar el mismo día o seguir
/// vigente (end_date nulo) no.
#[tokio::test]
async fn e7_la_experiencia_no_termina_antes_de_empezar() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let ins = |fin: &str, pos: i32| {
        format!(
            "INSERT INTO applicant_experiences \
             (id, applicant_id, company, title, start_date, end_date, position) \
             VALUES ('{}', '{a}', 'Acme', 'Dev', '2021-06-01', {fin}, {pos})",
            Uuid::now_v7()
        )
    };
    let err = error_de(db.exec(&ins("'2021-05-31'", 0)).await);
    assert!(err.contains("check"), "{err}");
    db.exec(&ins("'2021-06-01'", 1)).await.unwrap();
    db.exec(&ins("'2022-01-01'", 2)).await.unwrap();
    db.exec(&ins("NULL", 3)).await.unwrap();
    db.cleanup().await;
}

/// Extra: `professional_data` es 1:1 y sus FK a catálogos se verifican.
#[tokio::test]
async fn e8_professional_data_es_uno_a_uno_con_fk() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let ins = |nivel: &str| {
        format!(
            "INSERT INTO professional_data (id, applicant_id, last_academic_level_id) \
             VALUES ('{}', '{a}', {nivel})",
            Uuid::now_v7()
        )
    };
    let err = error_de(db.exec(&ins(&format!("'{}'", Uuid::now_v7()))).await);
    assert!(err.contains("foreign key"), "{err}");
    db.exec(&ins("NULL")).await.unwrap();
    assert!(db.exec(&ins("NULL")).await.is_err());
    db.cleanup().await;
}

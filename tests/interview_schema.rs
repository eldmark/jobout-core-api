//! Casos de verdad del esquema de sesiones de entrevista (E1–E10 de
//! docs/working/fase3-entrevista.md). Corren contra PostgreSQL real; ver `tests/common/mod.rs`.

mod common;

use common::{
    bytes_de, contiene, error_de, esquema, hex, llaves, nuevo_applicant, uuid_de, TestDb,
};
use jobout_core_api::interview::store_crypto::{decrypt_turn_text, encrypt_turn_text};
use migration::{Migrator, MigratorTrait};
use uuid::Uuid;

const TABLAS: [&str; 4] = [
    "interview_sessions",
    "interview_turns",
    "interview_scores",
    "applicant_level_progress",
];

async fn existe(db: &TestDb, tabla: &str) -> bool {
    db.count(&format!(
        "SELECT count(*) FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = '{tabla}'"
    ))
    .await
        == 1
}

/// (recruiter, nivel, versión de rúbrica vigente) sembrados por la migración del catálogo.
async fn catalogo(db: &TestDb) -> (Uuid, Uuid, Uuid) {
    let r = uuid_de(db, "SELECT id FROM ai_recruiters WHERE code = 'ana'").await;
    let l = uuid_de(db, "SELECT id FROM interview_levels WHERE code = 'junior'").await;
    let v = uuid_de(
        db,
        "SELECT rv.id FROM rubric_versions rv JOIN ai_recruiters a ON a.rubric_id = rv.rubric_id \
         WHERE a.code = 'ana' AND rv.is_current",
    )
    .await;
    (r, l, v)
}

async fn nueva_sesion(db: &TestDb, ap: Uuid, estado: &str) -> Result<Uuid, String> {
    let (r, l, v) = catalogo(db).await;
    let id = Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO interview_sessions (id, applicant_id, ai_recruiter_id, interview_level_id, \
           rubric_version_id, status) VALUES ('{id}', '{ap}', '{r}', '{l}', '{v}', '{estado}')"
    ))
    .await
    .map(|_| id)
    .map_err(|e| e.to_string())
}

async fn nuevo_turno(db: &TestDb, s: Uuid, seq: i32, texto: &str) -> Result<(), String> {
    let cifrado = encrypt_turn_text(&llaves(), texto).unwrap();
    db.exec(&format!(
        "INSERT INTO interview_turns (id, interview_session_id, seq, speaker, kind, transcript) \
         VALUES ('{}', '{s}', {seq}, 'applicant', 'answer', '\\x{}'::bytea)",
        Uuid::now_v7(),
        hex(&cifrado)
    ))
    .await
    .map_err(|e| e.to_string())
}

async fn nuevo_score(db: &TestDb, s: Uuid, score: i32, conf: &str) -> Result<(), String> {
    let (_, _, v) = catalogo(db).await;
    db.exec(&format!(
        "INSERT INTO interview_scores (id, interview_session_id, score, confidence, clarity, \
           breakdown, rubric_version_id) \
         VALUES ('{}', '{s}', {score}, {conf}, 70, '[{{\"code\":\"x\"}}]'::jsonb, '{v}')",
        Uuid::now_v7()
    ))
    .await
    .map_err(|e| e.to_string())
}

fn es_unico(err: &str) -> bool {
    err.contains("unique") || err.contains("duplicate")
}

/// E1: las tablas existen y `down` seguido de `up` es reversible.
#[tokio::test]
async fn e1_las_tablas_de_sesiones_son_reversibles() {
    let db = TestDb::migrated().await;
    for t in TABLAS {
        assert!(existe(&db, t).await, "falta la tabla {t}");
    }
    let antes = esquema(&db).await;
    Migrator::down(&db.conn, Some(8)).await.expect("down");
    for t in TABLAS {
        assert!(!existe(&db, t).await, "down debe quitar {t}");
    }
    for tipo in ["interview_status", "interview_speaker"] {
        assert_eq!(
            db.count(&format!(
                "SELECT count(*) FROM pg_type WHERE typname = '{tipo}'"
            ))
            .await,
            0,
            "down debe quitar el enum {tipo}"
        );
    }
    // El catálogo sigue ahí.
    assert!(existe(&db, "interview_levels").await);
    Migrator::up(&db.conn, None).await.expect("up");
    assert_eq!(esquema(&db).await, antes);
    db.cleanup().await;
}

/// E2: un estado fuera del enum falla; los cinco válidos entran.
#[tokio::test]
async fn e2_el_estado_fuera_del_enum_falla() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let err = nueva_sesion(&db, a, "bogus").await.unwrap_err();
    assert!(
        err.contains("enum") || err.contains("invalid input"),
        "{err}"
    );
    for e in ["created", "completed", "abandoned"] {
        nueva_sesion(&db, a, e).await.unwrap();
    }
    db.cleanup().await;
}

/// E3: una sola sesión in_progress/paused por candidato; las demás, las que se quiera.
#[tokio::test]
async fn e3_una_sola_sesion_activa_por_candidato() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let (_, b) = nuevo_applicant(&db, "b").await;
    nueva_sesion(&db, a, "in_progress").await.unwrap();
    assert!(es_unico(
        &nueva_sesion(&db, a, "in_progress").await.unwrap_err()
    ));
    assert!(es_unico(&nueva_sesion(&db, a, "paused").await.unwrap_err()));
    for e in [
        "completed",
        "completed",
        "abandoned",
        "abandoned",
        "created",
        "created",
    ] {
        nueva_sesion(&db, a, e).await.unwrap();
    }
    // Otro candidato tiene su propia sesión activa.
    nueva_sesion(&db, b, "paused").await.unwrap();
    assert!(es_unico(
        &nueva_sesion(&db, b, "in_progress").await.unwrap_err()
    ));
    db.cleanup().await;
}

/// E4: (sesión, seq) es único en los turnos.
#[tokio::test]
async fn e4_el_seq_es_unico_por_sesion() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let s1 = nueva_sesion(&db, a, "completed").await.unwrap();
    let s2 = nueva_sesion(&db, a, "completed").await.unwrap();
    nuevo_turno(&db, s1, 1, "hola").await.unwrap();
    assert!(es_unico(
        &nuevo_turno(&db, s1, 1, "otra").await.unwrap_err()
    ));
    nuevo_turno(&db, s1, 2, "hola").await.unwrap();
    // Otra sesión puede repetir el seq.
    nuevo_turno(&db, s2, 1, "hola").await.unwrap();
    db.cleanup().await;
}

/// E5: un solo score por sesión (1:1).
#[tokio::test]
async fn e5_un_solo_score_por_sesion() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let s1 = nueva_sesion(&db, a, "completed").await.unwrap();
    let s2 = nueva_sesion(&db, a, "completed").await.unwrap();
    nuevo_score(&db, s1, 80, "NULL").await.unwrap();
    assert!(es_unico(
        &nuevo_score(&db, s1, 50, "NULL").await.unwrap_err()
    ));
    nuevo_score(&db, s2, 50, "NULL").await.unwrap();
    db.cleanup().await;
}

/// E6: confidence acepta NULL; score, clarity y confidence fuera de 0–100 fallan; el desglose
/// es obligatorio.
#[tokio::test]
async fn e6_los_rangos_del_score_y_la_confianza_nula() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let ids = [
        nueva_sesion(&db, a, "completed").await.unwrap(),
        nueva_sesion(&db, a, "completed").await.unwrap(),
        nueva_sesion(&db, a, "completed").await.unwrap(),
        nueva_sesion(&db, a, "completed").await.unwrap(),
        nueva_sesion(&db, a, "completed").await.unwrap(),
        nueva_sesion(&db, a, "completed").await.unwrap(),
    ];
    nuevo_score(&db, ids[0], 0, "NULL").await.unwrap();
    nuevo_score(&db, ids[1], 100, "100").await.unwrap();
    for (s, score, conf) in [
        (ids[2], 101, "NULL"),
        (ids[3], -1, "NULL"),
        (ids[4], 50, "101"),
    ] {
        let err = nuevo_score(&db, s, score, conf).await.unwrap_err();
        assert!(err.contains("check"), "{err}");
    }
    let (_, _, v) = catalogo(&db).await;
    let sin = |col: &str, val: &str| {
        format!(
            "INSERT INTO interview_scores (id, interview_session_id, score, clarity, breakdown, \
               rubric_version_id) VALUES ('{}', '{}', 50, {col}, {val}, '{v}')",
            Uuid::now_v7(),
            ids[5]
        )
    };
    let err = error_de(db.exec(&sin("101", "'[]'::jsonb")).await);
    assert!(err.contains("check"), "{err}");
    let err = error_de(db.exec(&sin("50", "NULL")).await);
    assert!(err.contains("null"), "{err}");
    db.cleanup().await;
}

/// E7: la transcripción guardada no contiene el texto en claro.
#[tokio::test]
async fn e7_la_transcripcion_no_queda_en_claro() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let s = nueva_sesion(&db, a, "in_progress").await.unwrap();
    let secreto = "trabajo en Mercado Libre desde 2021";
    nuevo_turno(&db, s, 1, secreto).await.unwrap();
    let guardado = bytes_de(&db, "SELECT transcript FROM interview_turns").await;
    assert!(!contiene(&guardado, secreto.as_bytes()));
    assert!(!contiene(&guardado, b"Mercado"));
    assert_eq!(decrypt_turn_text(&llaves(), &guardado).unwrap(), secreto);
    // La columna es bytea: texto en claro ni siquiera cabe por tipo.
    let err = error_de(
        db.exec(&format!(
            "INSERT INTO interview_turns (id, interview_session_id, seq, speaker, kind, transcript) \
             VALUES ('{}', '{s}', 9, 'applicant', 'answer', 'hola'::text)",
            Uuid::now_v7()
        ))
        .await,
    );
    assert!(err.contains("bytea"), "{err}");
    db.cleanup().await;
}

/// E8: borrar al candidato borra en cascada sesiones, turnos, scores y progreso.
#[tokio::test]
async fn e8_borrar_el_candidato_borra_todo_en_cascada() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let (_, b) = nuevo_applicant(&db, "b").await;
    let (_, l, _) = catalogo(&db).await;
    let s = nueva_sesion(&db, a, "completed").await.unwrap();
    nuevo_turno(&db, s, 1, "hola").await.unwrap();
    nuevo_score(&db, s, 70, "NULL").await.unwrap();
    for ap in [a, b] {
        db.exec(&format!(
            "INSERT INTO applicant_level_progress (id, applicant_id, interview_level_id) \
             VALUES ('{}', '{ap}', '{l}')",
            Uuid::now_v7()
        ))
        .await
        .unwrap();
    }
    let sb = nueva_sesion(&db, b, "completed").await.unwrap();
    nuevo_turno(&db, sb, 1, "hola").await.unwrap();

    db.exec(&format!("DELETE FROM applicants WHERE id = '{a}'"))
        .await
        .unwrap();
    for (t, resto) in [
        ("interview_sessions", 1),
        ("interview_turns", 1),
        ("interview_scores", 0),
        ("applicant_level_progress", 1),
    ] {
        assert_eq!(
            db.count(&format!("SELECT count(*) FROM {t}")).await,
            resto,
            "{t}"
        );
    }
    db.cleanup().await;
}

/// E9: la versión de rúbrica de la sesión es obligatoria y debe existir; job_offer_id es opcional pero con FK.
#[tokio::test]
async fn e9_la_sesion_exige_una_version_de_rubrica_existente() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let (r, l, _) = catalogo(&db).await;
    let insertar = |v: &str| {
        format!(
            "INSERT INTO interview_sessions (id, applicant_id, ai_recruiter_id, \
               interview_level_id, rubric_version_id, status) \
             VALUES ('{}', '{a}', '{r}', '{l}', {v}, 'created')",
            Uuid::now_v7()
        )
    };
    let err = error_de(db.exec(&insertar("NULL")).await);
    assert!(err.contains("null"), "{err}");
    let err = error_de(db.exec(&insertar(&format!("'{}'", Uuid::now_v7()))).await);
    assert!(err.contains("foreign key"), "{err}");
    // Con las ofertas (Fase 5a, paso 4) `job_offer_id` tiene FK: sigue siendo opcional, pero un
    // id que no existe se rechaza (la FK con `ON DELETE SET NULL` se prueba en offers_schema).
    let s = nueva_sesion(&db, a, "created").await.unwrap();
    let err = error_de(
        db.exec(&format!(
            "UPDATE interview_sessions SET job_offer_id = '{}' WHERE id = '{s}'",
            Uuid::now_v7()
        ))
        .await,
    );
    assert!(err.contains("foreign key"), "{err}");
    db.cleanup().await;
}

/// E10: un solo progreso por (candidato, nivel); el contador no es negativo.
#[tokio::test]
async fn e10_el_progreso_es_unico_por_candidato_y_nivel() {
    let db = TestDb::migrated().await;
    let (_, a) = nuevo_applicant(&db, "a").await;
    let l1 = uuid_de(&db, "SELECT id FROM interview_levels WHERE code = 'junior'").await;
    let l2 = uuid_de(
        &db,
        "SELECT id FROM interview_levels WHERE code = 'startup'",
    )
    .await;
    let ins = |lvl: Uuid, extra: &str| {
        format!(
            "INSERT INTO applicant_level_progress (id, applicant_id, interview_level_id{}) \
             VALUES ('{}', '{a}', '{lvl}'{})",
            extra.split('|').next().unwrap(),
            Uuid::now_v7(),
            extra.split('|').nth(1).unwrap()
        )
    };
    db.exec(&ins(l1, "|")).await.unwrap();
    assert!(es_unico(&error_de(db.exec(&ins(l1, "|")).await)));
    db.exec(&ins(l2, "|")).await.unwrap();
    // Defaults y rangos.
    assert_eq!(
        db.count("SELECT count(*) FROM applicant_level_progress WHERE sessions_completed = 0 AND best_score IS NULL")
            .await,
        2
    );
    let (_, b) = nuevo_applicant(&db, "b").await;
    let malo = format!(
        "INSERT INTO applicant_level_progress (id, applicant_id, interview_level_id, best_score) \
         VALUES ('{}', '{b}', '{l1}', 101)",
        Uuid::now_v7()
    );
    assert!(error_de(db.exec(&malo).await).contains("check"));
    db.cleanup().await;
}

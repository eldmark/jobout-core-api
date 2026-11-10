//! Casos de verdad del cierre, el score y el desbloqueo (paso 8 de docs/working/fase3-entrevista.md):
//! C1–C11 y, para estas rutas, X1, X2, X3/X4 y X6. Contra PostgreSQL real; una base por prueba.
//!
//! Se escribieron antes que la implementación, en rojo. Ver CONTRIBUTING.md.

#[allow(dead_code)]
mod common;

use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc, Mutex,
};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use common::TestDb;
use http_body_util::BodyExt;
use jobout_core_api::{
    app_with_state,
    crypto::PiiKeys,
    interview::{
        store_crypto::encrypt_turn_text, CriterionResult, DeterministicEngine, EngineError,
        Evaluation, Improvement, InterviewEngine, Message, NextTurn, RecruiterConfig,
        RubricCriterion, SharedEngine,
    },
    interview_store,
    jwt::JwtKeys,
    state::AppState,
};
use sea_orm::{ConnectionTrait, Statement, TransactionTrait};
use serde_json::{json, Value};
use tower::ServiceExt;
use tracing_subscriber::layer::SubscriberExt;
use uuid::Uuid;

const CLAVE: &str = "una-clave-larga-123";
const MARCA: &str = "MARCA-7b1e-NO-DEBE-SALIR";

const BUENA: &str = "En mi proyecto final de la universidad usamos Rust con PostgreSQL y yo me encargué de las consultas lentas, porque la página tardaba 800 ms en responder. Primero medí el problema, después añadí un índice y reescribí dos consultas, y como resultado el tiempo bajó a 200 ms y atendimos a 300 usuarios a la vez.";

fn pii() -> Arc<PiiKeys> {
    Arc::new(
        PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
    )
}

fn jwt() -> Arc<JwtKeys> {
    Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap())
}

fn app_con(db: &TestDb, engine: SharedEngine) -> Router {
    app_with_state(AppState {
        db: db.conn.clone(),
        pii: pii(),
        jwt: jwt(),
        blobs: jobout_core_api::blob::memory_store(),
        engine,
    })
}

/// El motor determinista para conducir, pero con un score que la prueba fija y un contador de
/// evaluaciones: así los casos de desbloqueo no dependen de cómo puntúan las heurísticas.
struct MotorFijo {
    puntaje: Arc<AtomicU32>,
    evaluaciones: Arc<AtomicU32>,
    /// Criterios con los que se evaluó por última vez (códigos).
    vistos: Arc<Mutex<Vec<String>>>,
}

impl MotorFijo {
    fn nuevo(puntaje: u32) -> (SharedEngine, Arc<AtomicU32>, Arc<AtomicU32>) {
        let (e, p, n, _) = Self::con_vistos(puntaje);
        (e, p, n)
    }

    #[allow(clippy::type_complexity)]
    fn con_vistos(
        puntaje: u32,
    ) -> (
        SharedEngine,
        Arc<AtomicU32>,
        Arc<AtomicU32>,
        Arc<Mutex<Vec<String>>>,
    ) {
        let puntaje = Arc::new(AtomicU32::new(puntaje));
        let evaluaciones = Arc::new(AtomicU32::new(0));
        let vistos = Arc::new(Mutex::new(vec![]));
        let motor: SharedEngine = Arc::new(MotorFijo {
            puntaje: puntaje.clone(),
            evaluaciones: evaluaciones.clone(),
            vistos: vistos.clone(),
        });
        (motor, puntaje, evaluaciones, vistos)
    }
}

impl InterviewEngine for MotorFijo {
    fn next_turn(&self, c: &RecruiterConfig, h: &[Message]) -> Result<NextTurn, EngineError> {
        DeterministicEngine.next_turn(c, h)
    }

    fn evaluate(
        &self,
        _: &RecruiterConfig,
        t: &[Message],
        k: &[RubricCriterion],
    ) -> Result<Evaluation, EngineError> {
        if t.iter()
            .all(|m| m.speaker != jobout_core_api::interview::Speaker::Candidate)
        {
            return Err(EngineError::NoAnswers);
        }
        self.evaluaciones.fetch_add(1, Ordering::SeqCst);
        *self.vistos.lock().unwrap() = k.iter().map(|c| c.code.clone()).collect();
        let mut resto = self.puntaje.load(Ordering::SeqCst);
        let breakdown: Vec<CriterionResult> = k
            .iter()
            .map(|c| {
                let puntos = resto.min(c.max_points);
                resto -= puntos;
                CriterionResult {
                    code: c.code.clone(),
                    name: c.name.clone(),
                    points: puntos,
                    max_points: c.max_points,
                    why: format!("{}: explicación fija", c.name),
                }
            })
            .collect();
        let score = breakdown.iter().map(|b| b.points).sum();
        Ok(Evaluation {
            score,
            clarity: 50,
            improvements: breakdown
                .iter()
                .take(2)
                .map(|b| Improvement {
                    criterion_code: b.code.clone(),
                    text: format!("mejora de {}", b.code),
                    potential_points: b.max_points - b.points,
                })
                .collect(),
            breakdown,
            confidence: None,
        })
    }
}

/// Un motor cuya evaluación siempre falla.
struct MotorSinEvaluar;

impl InterviewEngine for MotorSinEvaluar {
    fn next_turn(&self, c: &RecruiterConfig, h: &[Message]) -> Result<NextTurn, EngineError> {
        DeterministicEngine.next_turn(c, h)
    }
    fn evaluate(
        &self,
        _: &RecruiterConfig,
        _: &[Message],
        _: &[RubricCriterion],
    ) -> Result<Evaluation, EngineError> {
        Err(EngineError::InvalidRubric)
    }
}

struct Respuesta {
    estado: StatusCode,
    cuerpo: Value,
}

async fn pedir(
    app: &Router,
    metodo: &str,
    uri: &str,
    token: Option<&str>,
    crudo: Option<String>,
) -> Respuesta {
    let mut peticion = Request::builder().method(metodo).uri(uri);
    if let Some(t) = token {
        peticion = peticion.header("authorization", format!("Bearer {t}"));
    }
    let cuerpo = match crudo {
        Some(c) => {
            peticion = peticion.header("content-type", "application/json");
            Body::from(c)
        }
        None => Body::empty(),
    };
    let r = app
        .clone()
        .oneshot(peticion.body(cuerpo).unwrap())
        .await
        .unwrap();
    let estado = r.status();
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    Respuesta {
        estado,
        cuerpo: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

async fn registrar(app: &Router, email: &str) -> String {
    let r = pedir(
        app,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(
            json!({"email": email, "password": CLAVE, "first_name": "Ana", "last_name": "Prueba"})
                .to_string(),
        ),
    )
    .await;
    assert_eq!(r.estado, StatusCode::CREATED, "{}", r.cuerpo);
    r.cuerpo["access_token"].as_str().unwrap().to_string()
}

struct Mundo {
    db: TestDb,
    app: Router,
    token: String,
    applicant: Uuid,
    sesion: Uuid,
}

async fn mundo_con(engine: SharedEngine) -> Mundo {
    let db = TestDb::migrated().await;
    let app = app_con(&db, engine);
    let token = registrar(&app, "ana.uno@correo-prueba.com").await;
    let (applicant, sesion) = sesion_nueva(&db, &app, &token, "ana", "junior").await;
    Mundo {
        db,
        app,
        token,
        applicant,
        sesion,
    }
}

async fn mundo() -> Mundo {
    mundo_con(Arc::new(DeterministicEngine)).await
}

/// Crea una sesión por la API real. Devuelve (applicant, sesión).
async fn sesion_nueva(
    db: &TestDb,
    app: &Router,
    token: &str,
    recruiter: &str,
    nivel: &str,
) -> (Uuid, Uuid) {
    let (applicant, id) = intentar_sesion(db, app, token, recruiter, nivel).await;
    (
        applicant,
        id.unwrap_or_else(|| panic!("no se pudo crear la sesión de {nivel}")),
    )
}

async fn intentar_sesion(
    db: &TestDb,
    app: &Router,
    token: &str,
    recruiter: &str,
    nivel: &str,
) -> (Uuid, Option<Uuid>) {
    let user = jwt().validate(token).unwrap();
    let applicant = common::uuid_de(
        db,
        &format!("SELECT id FROM applicants WHERE user_id = '{user}'"),
    )
    .await;
    let r = common::uuid_de(
        db,
        &format!("SELECT id FROM ai_recruiters WHERE code = '{recruiter}'"),
    )
    .await;
    let n = common::uuid_de(
        db,
        &format!("SELECT id FROM interview_levels WHERE code = '{nivel}'"),
    )
    .await;
    let c = pedir(
        app,
        "POST",
        "/api/v1/interviews",
        Some(token),
        Some(json!({"recruiter_id": r, "level_id": n}).to_string()),
    )
    .await;
    let id = (c.estado == StatusCode::CREATED)
        .then(|| c.cuerpo["id"].as_str().unwrap().parse().unwrap());
    (applicant, id)
}

fn url_finish(id: impl std::fmt::Display) -> String {
    format!("/api/v1/interviews/{id}/finish")
}

fn url_score(id: impl std::fmt::Display) -> String {
    format!("/api/v1/interviews/{id}/score")
}

async fn terminar(w: &Mundo) -> Respuesta {
    pedir(&w.app, "POST", &url_finish(w.sesion), Some(&w.token), None).await
}

async fn score_de(w: &Mundo) -> Respuesta {
    pedir(&w.app, "GET", &url_score(w.sesion), Some(&w.token), None).await
}

async fn sql_uno<T: sea_orm::TryGetable>(db: &TestDb, sql: &str) -> T {
    db.conn
        .query_one(Statement::from_string(
            db.conn.get_database_backend(),
            sql.to_string(),
        ))
        .await
        .unwrap()
        .expect("una fila")
        .try_get_by_index::<T>(0)
        .unwrap()
}

async fn estado_de(db: &TestDb, id: Uuid) -> String {
    sql_uno(
        db,
        &format!("SELECT status::text FROM interview_sessions WHERE id = '{id}'"),
    )
    .await
}

async fn n_scores(db: &TestDb, sesion: Uuid) -> i64 {
    db.count(&format!(
        "SELECT count(*) FROM interview_scores WHERE interview_session_id = '{sesion}'"
    ))
    .await
}

async fn n_scores_total(db: &TestDb) -> i64 {
    db.count("SELECT count(*) FROM interview_scores").await
}

/// (best_score, sessions_completed) del progreso de la persona en el nivel, si hay fila.
async fn progreso(db: &TestDb, applicant: Uuid, nivel: &str) -> Option<(Option<i16>, i32)> {
    let f = db
        .conn
        .query_one(Statement::from_string(
            db.conn.get_database_backend(),
            format!(
                "SELECT p.best_score, p.sessions_completed FROM applicant_level_progress p \
                 JOIN interview_levels l ON l.id = p.interview_level_id \
                 WHERE p.applicant_id = '{applicant}' AND l.code = '{nivel}' AND p.unlocked_at IS NOT NULL"
            ),
        ))
        .await
        .unwrap()?;
    Some((
        f.try_get_by_index(0).unwrap(),
        f.try_get_by_index(1).unwrap(),
    ))
}

async fn n_progreso(db: &TestDb, applicant: Uuid) -> i64 {
    db.count(&format!(
        "SELECT count(*) FROM applicant_level_progress WHERE applicant_id = '{applicant}'"
    ))
    .await
}

async fn sembrar_turno(
    db: &TestDb,
    sesion: Uuid,
    seq: i32,
    speaker: &str,
    kind: &str,
    texto: &str,
) {
    let cifrado = encrypt_turn_text(&common::llaves(), texto).unwrap();
    db.exec(&format!(
        "INSERT INTO interview_turns (id, interview_session_id, seq, speaker, kind, transcript) \
         VALUES ('{}', '{sesion}', {seq}, '{speaker}'::interview_speaker, '{kind}', '\\x{}'::bytea)",
        Uuid::now_v7(),
        common::hex(&cifrado)
    ))
    .await
    .unwrap();
}

/// Añade `n` intercambios (respuesta de la persona + pregunta del recruiter) al final de la sesión.
async fn sembrar_respuestas_con(db: &TestDb, sesion: Uuid, n: i32, texto: &str) {
    let mut seq: i32 = db
        .count(&format!("SELECT COALESCE(max(seq), 0)::bigint FROM interview_turns WHERE interview_session_id = '{sesion}'"))
        .await as i32;
    for _ in 0..n {
        sembrar_turno(db, sesion, seq + 1, "applicant", "answer", texto).await;
        sembrar_turno(
            db,
            sesion,
            seq + 2,
            "recruiter",
            "question",
            "otra pregunta",
        )
        .await;
        seq += 2;
    }
}

async fn sembrar_respuestas(db: &TestDb, sesion: Uuid, n: i32) {
    sembrar_respuestas_con(db, sesion, n, BUENA).await;
}

/// Contesta lo suficiente y termina la sesión actual de `w` por la API. Devuelve la respuesta.
async fn contestar_y_terminar(w: &Mundo, sesion: Uuid) -> Respuesta {
    sembrar_respuestas(&w.db, sesion, 2).await;
    pedir(&w.app, "POST", &url_finish(sesion), Some(&w.token), None).await
}

fn error_de(c: &Value) -> &str {
    c["error"].as_str().unwrap_or("")
}

// ================================================================== C1, C3

/// C1: con al menos 2 respuestas, `finish` calcula el score, cierra la sesión y lo devuelve con
/// la forma exacta del contrato.
#[tokio::test]
async fn c1_finish_calcula_el_score_y_completa_la_sesion() {
    let w = mundo().await;
    sembrar_respuestas(&w.db, w.sesion, 2).await;
    let r = terminar(&w).await;
    assert_eq!(r.estado, StatusCode::OK, "{}", r.cuerpo);
    let c = &r.cuerpo;
    assert_eq!(
        c.as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        [
            "score",
            "clarity",
            "confidence",
            "breakdown",
            "improvements",
            "rubric_version",
            "computed_at"
        ]
        .into_iter()
        .map(String::from)
        .collect()
    );
    let score = c["score"].as_i64().unwrap();
    assert!((0..=100).contains(&score));
    assert!((0..=100).contains(&c["clarity"].as_i64().unwrap()));
    let desglose = c["breakdown"].as_array().unwrap();
    assert_eq!(desglose.len(), 5);
    let mut suma = 0;
    for d in desglose {
        assert_eq!(d.as_object().unwrap().len(), 5, "{d}");
        assert!(d["criterion"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(d["label"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(d["why"].as_str().is_some_and(|s| !s.is_empty()));
        let (p, m) = (
            d["score"].as_i64().unwrap(),
            d["max_points"].as_i64().unwrap(),
        );
        assert!(p <= m && m > 0);
        suma += p;
    }
    assert_eq!(suma, score, "la suma del desglose es el score");
    assert_eq!(
        desglose
            .iter()
            .map(|d| d["max_points"].as_i64().unwrap())
            .sum::<i64>(),
        100
    );
    let mejoras = c["improvements"].as_array().unwrap();
    assert!((2..=3).contains(&mejoras.len()));
    assert!(mejoras
        .iter()
        .all(|m| m.as_str().is_some_and(|s| !s.is_empty())));

    assert_eq!(estado_de(&w.db, w.sesion).await, "completed");
    assert_eq!(
        w.db.count(&format!(
            "SELECT count(*) FROM interview_sessions WHERE id = '{}' AND completed_at IS NOT NULL",
            w.sesion
        ))
        .await,
        1
    );
    assert_eq!(n_scores(&w.db, w.sesion).await, 1);
    // Lo persistido es lo devuelto.
    assert_eq!(
        sql_uno::<i16>(
            &w.db,
            &format!(
                "SELECT score FROM interview_scores WHERE interview_session_id = '{}'",
                w.sesion
            )
        )
        .await as i64,
        score
    );
    w.db.cleanup().await;
}

/// C3: `GET /score` devuelve `score`, `clarity`, `confidence = null`, el desglose, la versión de la
/// rúbrica y `computed_at`: lo mismo que devolvió `finish`.
#[tokio::test]
async fn c3_get_score_devuelve_el_mismo_cuerpo_que_finish() {
    let w = mundo().await;
    sembrar_respuestas(&w.db, w.sesion, 2).await;
    let f = terminar(&w).await;
    assert_eq!(f.estado, StatusCode::OK, "{}", f.cuerpo);
    let g = score_de(&w).await;
    assert_eq!(g.estado, StatusCode::OK, "{}", g.cuerpo);
    assert_eq!(g.cuerpo, f.cuerpo);
    assert!(g.cuerpo["confidence"].is_null());
    assert_eq!(g.cuerpo["rubric_version"], 1);
    let c = g.cuerpo["computed_at"].as_str().unwrap();
    assert!(
        c.contains('T') && (c.ends_with('Z') || c.contains('+')),
        "{c}"
    );
    assert_eq!(
        sql_uno::<Option<i16>>(&w.db, "SELECT confidence FROM interview_scores").await,
        None
    );
    w.db.cleanup().await;
}

// ================================================================== C2, C4 y estados

/// C2: con menos de 2 respuestas → 422 `entrevista_muy_corta`; la sesión sigue abierta, sin score
/// ni progreso; y con la segunda respuesta ya se puede terminar.
#[tokio::test]
async fn c2_con_menos_de_dos_respuestas_no_se_termina() {
    let (motor, _, _) = MotorFijo::nuevo(80);
    let w = mundo_con(motor).await;
    for n in 0..2 {
        let r = terminar(&w).await;
        assert_eq!(
            r.estado,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{n} respuestas: {}",
            r.cuerpo
        );
        assert_eq!(error_de(&r.cuerpo), "entrevista_muy_corta");
        assert!(r.cuerpo["message"].as_str().is_some_and(|m| !m.is_empty()));
        assert_eq!(estado_de(&w.db, w.sesion).await, "in_progress");
        assert_eq!(n_scores_total(&w.db).await, 0);
        assert_eq!(n_progreso(&w.db, w.applicant).await, 0);
        sembrar_respuestas(&w.db, w.sesion, 1).await;
    }
    // Ahora hay 2 respuestas.
    assert_eq!(terminar(&w).await.estado, StatusCode::OK);
    assert_eq!(estado_de(&w.db, w.sesion).await, "completed");
    w.db.cleanup().await;
}

/// C2 (borde): las preguntas del recruiter no cuentan como respuestas.
#[tokio::test]
async fn c2_solo_cuentan_las_respuestas_de_la_persona() {
    let w = mundo().await;
    // Varios turnos del recruiter y una sola respuesta.
    sembrar_respuestas(&w.db, w.sesion, 1).await;
    sembrar_turno(&w.db, w.sesion, 4, "recruiter", "follow_up", "y entonces?").await;
    let r = terminar(&w).await;
    assert_eq!(r.estado, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.cuerpo);
    w.db.cleanup().await;
}

/// C4: el score de una sesión no terminada (en curso, pausada, abandonada) → 409 `estado_invalido`.
#[tokio::test]
async fn c4_el_score_de_una_sesion_no_terminada_da_409() {
    let w = mundo().await;
    for estado in ["in_progress", "paused", "abandoned"] {
        w.db.exec(&format!(
            "UPDATE interview_sessions SET status = '{estado}' WHERE id = '{}'",
            w.sesion
        ))
        .await
        .unwrap();
        let r = score_de(&w).await;
        assert_eq!(r.estado, StatusCode::CONFLICT, "{estado}");
        assert_eq!(error_de(&r.cuerpo), "estado_invalido");
    }
    w.db.cleanup().await;
}

/// `finish` desde `paused` o `abandoned` → 409 `estado_invalido`, sin tocar nada.
#[tokio::test]
async fn finish_desde_pausada_o_abandonada_da_409_sin_tocar_nada() {
    let w = mundo().await;
    sembrar_respuestas(&w.db, w.sesion, 2).await;
    for estado in ["paused", "abandoned"] {
        w.db.exec(&format!(
            "UPDATE interview_sessions SET status = '{estado}' WHERE id = '{}'",
            w.sesion
        ))
        .await
        .unwrap();
        let r = terminar(&w).await;
        assert_eq!(r.estado, StatusCode::CONFLICT, "{estado}: {}", r.cuerpo);
        assert_eq!(error_de(&r.cuerpo), "estado_invalido");
        assert_eq!(estado_de(&w.db, w.sesion).await, estado);
        assert_eq!(n_scores_total(&w.db).await, 0);
        assert_eq!(n_progreso(&w.db, w.applicant).await, 0);
    }
    w.db.cleanup().await;
}

// ================================================================== C5

/// C5: el desglose se calcula con la rúbrica **fijada por la sesión**, no con la vigente hoy:
/// tras publicar otra versión con otros criterios, el score sigue siendo de la versión 1.
#[tokio::test]
async fn c5_se_usa_la_rubrica_de_la_sesion_no_la_vigente() {
    let (motor, _, _, vistos) = MotorFijo::con_vistos(70);
    let w = mundo_con(motor).await;
    let ana = "(SELECT rubric_id FROM ai_recruiters WHERE code = 'ana')";
    w.db.exec(&format!(
        "UPDATE rubric_versions SET is_current = false WHERE rubric_id = {ana} AND is_current"
    ))
    .await
    .unwrap();
    w.db.exec(&format!(
        "INSERT INTO rubric_versions (id, rubric_id, version, criteria, published_at, is_current) \
         VALUES ('{}', {ana}, 2, \
         '[{{\"code\":\"solo_a\",\"name\":\"Solo A\",\"max_points\":60,\"description\":\"a\"}},\
           {{\"code\":\"solo_b\",\"name\":\"Solo B\",\"max_points\":40,\"description\":\"b\"}}]'::jsonb, now(), true)",
        Uuid::now_v7()
    ))
    .await
    .unwrap();
    sembrar_respuestas(&w.db, w.sesion, 2).await;
    let r = terminar(&w).await;
    assert_eq!(r.estado, StatusCode::OK, "{}", r.cuerpo);
    assert_eq!(r.cuerpo["rubric_version"], 1);
    let codigos: Vec<&str> = r.cuerpo["breakdown"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["criterion"].as_str().unwrap())
        .collect();
    assert_eq!(
        codigos,
        [
            "especificidad",
            "estructura",
            "relevancia",
            "claridad",
            "profundidad"
        ]
    );
    assert_eq!(*vistos.lock().unwrap(), codigos);
    assert_eq!(
        w.db.count(&format!(
            "SELECT count(*) FROM interview_scores sc JOIN rubric_versions v ON v.id = sc.rubric_version_id \
             WHERE sc.interview_session_id = '{}' AND v.version = 1",
            w.sesion
        ))
        .await,
        1
    );
    // Y la sesión siguiente, empezada con la versión 2, usa la 2.
    let (_, otra) = sesion_nueva(&w.db, &w.app, &w.token, "ana", "junior").await;
    sembrar_respuestas(&w.db, otra, 2).await;
    let r = pedir(&w.app, "POST", &url_finish(otra), Some(&w.token), None).await;
    assert_eq!(r.estado, StatusCode::OK, "{}", r.cuerpo);
    assert_eq!(r.cuerpo["rubric_version"], 2);
    assert_eq!(r.cuerpo["breakdown"].as_array().unwrap().len(), 2);
    w.db.cleanup().await;
}

// ================================================================== done = completed con score

/// T8/C1: cuando el motor cierra la sesión (`done: true`) ya hay score; también con el motor real.
#[tokio::test]
async fn done_true_implica_completada_con_score() {
    let w = mundo().await;
    let mut hecho = false;
    for _ in 0..12 {
        let r = pedir(
            &w.app,
            "POST",
            &format!("/api/v1/interviews/{}/turns", w.sesion),
            Some(&w.token),
            Some(json!({"text": BUENA}).to_string()),
        )
        .await;
        assert_eq!(r.estado, StatusCode::CREATED, "{}", r.cuerpo);
        if r.cuerpo["done"] == true {
            hecho = true;
            break;
        }
    }
    assert!(hecho);
    assert_eq!(estado_de(&w.db, w.sesion).await, "completed");
    assert_eq!(n_scores(&w.db, w.sesion).await, 1);
    let g = score_de(&w).await;
    assert_eq!(g.estado, StatusCode::OK, "{}", g.cuerpo);
    assert_eq!(g.cuerpo["rubric_version"], 1);
    assert_eq!(
        progreso(&w.db, w.applicant, "junior").await.map(|p| p.1),
        Some(1)
    );
    // Terminar después es idempotente: mismo score, sin duplicar.
    let f = terminar(&w).await;
    assert_eq!(f.estado, StatusCode::OK);
    assert_eq!(f.cuerpo, g.cuerpo);
    assert_eq!(n_scores(&w.db, w.sesion).await, 1);
    assert_eq!(
        progreso(&w.db, w.applicant, "junior").await.map(|p| p.1),
        Some(1)
    );
    w.db.cleanup().await;
}

/// El tope de 40 turnos también cierra con score.
#[tokio::test]
async fn el_tope_de_40_turnos_cierra_con_score() {
    let w = mundo().await;
    sembrar_respuestas(&w.db, w.sesion, 18).await; // 37 turnos
    let r = pedir(
        &w.app,
        "POST",
        &format!("/api/v1/interviews/{}/turns", w.sesion),
        Some(&w.token),
        Some(json!({"text": BUENA}).to_string()),
    )
    .await;
    assert_eq!(r.estado, StatusCode::CREATED, "{}", r.cuerpo);
    assert_eq!(r.cuerpo["done"], true);
    assert_eq!(estado_de(&w.db, w.sesion).await, "completed");
    assert_eq!(n_scores(&w.db, w.sesion).await, 1);
    w.db.cleanup().await;
}

/// Si la evaluación falla al cerrar por turno, todo se deshace: el turno no queda y la sesión
/// sigue abierta (`done: true` nunca significa «completada sin score»).
#[tokio::test]
async fn si_evaluar_falla_el_turno_de_cierre_se_deshace() {
    let w = mundo_con(Arc::new(MotorSinEvaluar)).await;
    sembrar_respuestas(&w.db, w.sesion, 18).await;
    let antes =
        w.db.count(&format!(
            "SELECT count(*) FROM interview_turns WHERE interview_session_id = '{}'",
            w.sesion
        ))
        .await;
    let r = pedir(
        &w.app,
        "POST",
        &format!("/api/v1/interviews/{}/turns", w.sesion),
        Some(&w.token),
        Some(json!({"text": BUENA}).to_string()),
    )
    .await;
    assert_eq!(r.estado, StatusCode::SERVICE_UNAVAILABLE, "{}", r.cuerpo);
    assert_eq!(error_de(&r.cuerpo), "motor_no_disponible");
    assert_eq!(estado_de(&w.db, w.sesion).await, "in_progress");
    assert_eq!(
        w.db.count(&format!(
            "SELECT count(*) FROM interview_turns WHERE interview_session_id = '{}'",
            w.sesion
        ))
        .await,
        antes
    );
    assert_eq!(n_scores_total(&w.db).await, 0);
    w.db.cleanup().await;
}

/// Si la evaluación falla en `finish` → 503 `motor_no_disponible` y la sesión sigue abierta.
#[tokio::test]
async fn si_evaluar_falla_finish_da_503_y_no_cierra() {
    let w = mundo_con(Arc::new(MotorSinEvaluar)).await;
    sembrar_respuestas(&w.db, w.sesion, 2).await;
    let r = terminar(&w).await;
    assert_eq!(r.estado, StatusCode::SERVICE_UNAVAILABLE, "{}", r.cuerpo);
    assert_eq!(error_de(&r.cuerpo), "motor_no_disponible");
    assert_eq!(estado_de(&w.db, w.sesion).await, "in_progress");
    assert_eq!(n_scores_total(&w.db).await, 0);
    assert_eq!(n_progreso(&w.db, w.applicant).await, 0);
    w.db.cleanup().await;
}

// ================================================================== C6, C7

/// C6: terminar dos veces es idempotente: mismo cuerpo, una sola evaluación, un solo score y el
/// progreso no se cuenta dos veces.
#[tokio::test]
async fn c6_terminar_dos_veces_no_recalcula_ni_duplica() {
    let (motor, puntaje, evaluaciones) = MotorFijo::nuevo(70);
    let w = mundo_con(motor).await;
    sembrar_respuestas(&w.db, w.sesion, 2).await;
    let a = terminar(&w).await;
    assert_eq!(a.estado, StatusCode::OK, "{}", a.cuerpo);
    puntaje.store(10, Ordering::SeqCst); // si recalculara, saldría otro número
    let b = terminar(&w).await;
    assert_eq!(b.estado, StatusCode::OK, "{}", b.cuerpo);
    assert_eq!(a.cuerpo, b.cuerpo);
    assert_eq!(a.cuerpo["score"], 70);
    assert_eq!(evaluaciones.load(Ordering::SeqCst), 1);
    assert_eq!(n_scores(&w.db, w.sesion).await, 1);
    assert_eq!(
        progreso(&w.db, w.applicant, "junior").await,
        Some((Some(70), 1))
    );
    w.db.cleanup().await;
}

/// C6/C7: el gancho de cierre, llamado dos veces sobre la misma sesión, deja un solo score y no
/// cuenta dos veces: el índice único del score es la red de seguridad.
#[tokio::test]
async fn c6_el_gancho_de_cierre_es_idempotente() {
    let (motor, _, evaluaciones) = MotorFijo::nuevo(65);
    let w = mundo_con(motor.clone()).await;
    sembrar_respuestas(&w.db, w.sesion, 2).await;
    w.db.exec(&format!(
        "UPDATE interview_sessions SET status = 'completed', completed_at = now() WHERE id = '{}'",
        w.sesion
    ))
    .await
    .unwrap();
    for _ in 0..2 {
        interview_store::on_session_completed(
            &w.db.conn,
            &common::llaves(),
            motor.as_ref(),
            w.applicant,
            w.sesion,
        )
        .await
        .expect("el gancho repetido no falla");
    }
    assert_eq!(n_scores(&w.db, w.sesion).await, 1);
    assert_eq!(
        progreso(&w.db, w.applicant, "junior").await,
        Some((Some(65), 1))
    );
    assert_eq!(
        evaluaciones.load(Ordering::SeqCst),
        2,
        "evalúa, pero no inserta de nuevo"
    );
    w.db.cleanup().await;
}

/// C7: varios `finish` simultáneos dejan **un solo** score y un solo conteo; cada uno recibe 200
/// (el que terminó o uno que ya la encontró completada) o 409 `turno_en_proceso` (el candado).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c7_finish_simultaneos_dejan_un_solo_score() {
    let (motor, _, evaluaciones) = MotorFijo::nuevo(75);
    let w = mundo_con(motor).await;
    sembrar_respuestas(&w.db, w.sesion, 2).await;
    let mut tareas = vec![];
    for _ in 0..8 {
        let (app, token, id) = (w.app.clone(), w.token.clone(), w.sesion);
        tareas.push(tokio::spawn(async move {
            pedir(&app, "POST", &url_finish(id), Some(&token), None).await
        }));
    }
    let mut cuerpos_ok = vec![];
    for t in tareas {
        let r = t.await.unwrap();
        match r.estado {
            StatusCode::OK => cuerpos_ok.push(r.cuerpo),
            StatusCode::CONFLICT => assert_eq!(error_de(&r.cuerpo), "turno_en_proceso"),
            otro => panic!("estado inesperado {otro}: {}", r.cuerpo),
        }
    }
    assert!(!cuerpos_ok.is_empty(), "al menos uno termina");
    assert!(
        cuerpos_ok.windows(2).all(|p| p[0] == p[1]),
        "todos ven el mismo score"
    );
    assert_eq!(n_scores(&w.db, w.sesion).await, 1);
    assert_eq!(evaluaciones.load(Ordering::SeqCst), 1);
    assert_eq!(
        progreso(&w.db, w.applicant, "junior").await,
        Some((Some(75), 1))
    );
    w.db.cleanup().await;
}

/// C7 (determinista): con la fila de la sesión bloqueada por otra transacción, `finish` no espera:
/// 409 `turno_en_proceso`, sin tocar nada; al soltar el candado, termina.
#[tokio::test]
async fn c7_finish_con_la_sesion_bloqueada_da_409_turno_en_proceso() {
    let w = mundo().await;
    sembrar_respuestas(&w.db, w.sesion, 2).await;
    let txn = w.db.conn.begin().await.unwrap();
    txn.execute(Statement::from_string(
        txn.get_database_backend(),
        format!(
            "SELECT 1 FROM interview_sessions WHERE id = '{}' FOR UPDATE",
            w.sesion
        ),
    ))
    .await
    .unwrap();
    let r = terminar(&w).await;
    assert_eq!(r.estado, StatusCode::CONFLICT, "{}", r.cuerpo);
    assert_eq!(error_de(&r.cuerpo), "turno_en_proceso");
    txn.rollback().await.unwrap();
    assert_eq!(estado_de(&w.db, w.sesion).await, "in_progress");
    assert_eq!(n_scores_total(&w.db).await, 0);
    assert_eq!(terminar(&w).await.estado, StatusCode::OK);
    w.db.cleanup().await;
}

// ================================================================== C8, C9, C10, C11

/// C8: terminar con score >= `min_score_to_unlock` del siguiente nivel lo desbloquea (justo en el
/// mínimo también); con eso se puede empezar una sesión de Startup.
#[tokio::test]
async fn c8_un_score_en_el_minimo_desbloquea_el_siguiente_nivel() {
    let (motor, _, _) = MotorFijo::nuevo(60);
    let w = mundo_con(motor).await;
    assert!(progreso(&w.db, w.applicant, "startup").await.is_none());
    // Antes de desbloquear, Startup está bloqueado.
    let r = contestar_y_terminar(&w, w.sesion).await;
    assert_eq!(r.estado, StatusCode::OK, "{}", r.cuerpo);
    assert_eq!(r.cuerpo["score"], 60);
    assert_eq!(
        progreso(&w.db, w.applicant, "junior").await,
        Some((Some(60), 1))
    );
    assert_eq!(
        progreso(&w.db, w.applicant, "startup").await,
        Some((None, 0))
    );
    // Y ya se puede empezar.
    let (_, id) = intentar_sesion(&w.db, &w.app, &w.token, "ana", "startup").await;
    assert!(id.is_some(), "Startup debía poder empezarse");
    w.db.cleanup().await;
}

/// C9: por debajo del mínimo no se desbloquea nada; un score mayor sube `best_score` y uno menor
/// no lo baja.
#[tokio::test]
async fn c9_por_debajo_no_desbloquea_y_best_score_solo_sube() {
    let (motor, puntaje, _) = MotorFijo::nuevo(59);
    let w = mundo_con(motor).await;
    let r = contestar_y_terminar(&w, w.sesion).await;
    assert_eq!(r.estado, StatusCode::OK, "{}", r.cuerpo);
    assert_eq!(
        progreso(&w.db, w.applicant, "junior").await,
        Some((Some(59), 1))
    );
    assert!(
        progreso(&w.db, w.applicant, "startup").await.is_none(),
        "59 < 60: no desbloquea"
    );
    let (_, id) = intentar_sesion(&w.db, &w.app, &w.token, "ana", "startup").await;
    assert!(id.is_none(), "Startup sigue bloqueado");

    // Un score mayor.
    puntaje.store(85, Ordering::SeqCst);
    let (_, s2) = sesion_nueva(&w.db, &w.app, &w.token, "ana", "junior").await;
    contestar_y_terminar(&w, s2).await;
    assert_eq!(
        progreso(&w.db, w.applicant, "junior").await,
        Some((Some(85), 2))
    );
    assert!(progreso(&w.db, w.applicant, "startup").await.is_some());

    // Uno menor: no baja el mejor, ni quita lo desbloqueado, pero sí cuenta la sesión.
    puntaje.store(20, Ordering::SeqCst);
    let (_, s3) = sesion_nueva(&w.db, &w.app, &w.token, "ana", "junior").await;
    contestar_y_terminar(&w, s3).await;
    assert_eq!(
        progreso(&w.db, w.applicant, "junior").await,
        Some((Some(85), 3))
    );
    assert!(progreso(&w.db, w.applicant, "startup").await.is_some());
    w.db.cleanup().await;
}

/// C10: `sessions_completed` solo cuenta sesiones `completed`: ni las abandonadas, ni las
/// pausadas, ni un `finish` rechazado por corto.
#[tokio::test]
async fn c10_sessions_completed_solo_cuenta_las_completadas() {
    let (motor, _, _) = MotorFijo::nuevo(40);
    let w = mundo_con(motor).await;
    // Una corta rechazada y luego abandonada.
    assert_eq!(terminar(&w).await.estado, StatusCode::UNPROCESSABLE_ENTITY);
    let a = pedir(
        &w.app,
        "POST",
        &format!("/api/v1/interviews/{}/abandon", w.sesion),
        Some(&w.token),
        None,
    )
    .await;
    assert_eq!(a.estado, StatusCode::NO_CONTENT);
    assert!(progreso(&w.db, w.applicant, "junior").await.is_none());
    // Una completada.
    let (_, s2) = sesion_nueva(&w.db, &w.app, &w.token, "ana", "junior").await;
    contestar_y_terminar(&w, s2).await;
    assert_eq!(
        progreso(&w.db, w.applicant, "junior").await,
        Some((Some(40), 1))
    );
    // Otra abandonada con respuestas, y otra pausada: siguen en 1.
    let (_, s3) = sesion_nueva(&w.db, &w.app, &w.token, "ana", "junior").await;
    sembrar_respuestas(&w.db, s3, 3).await;
    pedir(
        &w.app,
        "POST",
        &format!("/api/v1/interviews/{s3}/abandon"),
        Some(&w.token),
        None,
    )
    .await;
    let (_, s4) = sesion_nueva(&w.db, &w.app, &w.token, "ana", "junior").await;
    pedir(
        &w.app,
        "POST",
        &format!("/api/v1/interviews/{s4}/pause"),
        Some(&w.token),
        None,
    )
    .await;
    assert_eq!(
        progreso(&w.db, w.applicant, "junior").await.map(|p| p.1),
        Some(1)
    );
    // Borrar una completada no cambia lo acumulado, y la siguiente suma sobre eso.
    pedir(
        &w.app,
        "DELETE",
        &format!("/api/v1/interviews/{s2}"),
        Some(&w.token),
        None,
    )
    .await;
    assert_eq!(
        progreso(&w.db, w.applicant, "junior").await.map(|p| p.1),
        Some(1)
    );
    pedir(
        &w.app,
        "POST",
        &format!("/api/v1/interviews/{s4}/abandon"),
        Some(&w.token),
        None,
    )
    .await;
    let (_, s5) = sesion_nueva(&w.db, &w.app, &w.token, "ana", "junior").await;
    contestar_y_terminar(&w, s5).await;
    assert_eq!(
        progreso(&w.db, w.applicant, "junior").await.map(|p| p.1),
        Some(2)
    );
    w.db.cleanup().await;
}

/// C11: un escalón por vez. Terminar Junior con un score altísimo desbloquea Startup y nada más,
/// aunque Amazon y Google estuvieran disponibles y el score alcanzara sus mínimos.
#[tokio::test]
async fn c11_desbloquear_startup_no_desbloquea_amazon() {
    let (motor, _, _) = MotorFijo::nuevo(100);
    let w = mundo_con(motor).await;
    w.db.exec("UPDATE interview_levels SET available = true")
        .await
        .unwrap();
    let r = contestar_y_terminar(&w, w.sesion).await;
    assert_eq!(r.estado, StatusCode::OK, "{}", r.cuerpo);
    assert!(progreso(&w.db, w.applicant, "startup").await.is_some());
    for otro in ["amazon", "google", "mckinsey"] {
        assert!(
            progreso(&w.db, w.applicant, otro).await.is_none(),
            "{otro} no debía desbloquearse"
        );
    }
    assert_eq!(n_progreso(&w.db, w.applicant).await, 2);
    // Y terminar Startup desbloquea Amazon, no Google.
    let (_, s) = sesion_nueva(&w.db, &w.app, &w.token, "ana", "startup").await;
    contestar_y_terminar(&w, s).await;
    assert_eq!(
        progreso(&w.db, w.applicant, "startup").await,
        Some((Some(100), 1))
    );
    assert!(progreso(&w.db, w.applicant, "amazon").await.is_some());
    assert!(progreso(&w.db, w.applicant, "google").await.is_none());
    w.db.cleanup().await;
}

/// Un nivel siguiente que no está `available` no se desbloquea nunca (R5), aunque el score alcance.
#[tokio::test]
async fn un_nivel_no_disponible_no_se_desbloquea() {
    let (motor, _, _) = MotorFijo::nuevo(100);
    let w = mundo_con(motor).await;
    let r = contestar_y_terminar(&w, w.sesion).await;
    assert_eq!(r.estado, StatusCode::OK, "{}", r.cuerpo);
    let (_, s) = sesion_nueva(&w.db, &w.app, &w.token, "ana", "startup").await;
    contestar_y_terminar(&w, s).await;
    assert_eq!(
        progreso(&w.db, w.applicant, "startup").await,
        Some((Some(100), 1))
    );
    assert!(
        progreso(&w.db, w.applicant, "amazon").await.is_none(),
        "Amazon no está disponible"
    );
    w.db.cleanup().await;
}

// ================================================================== X1, X2, X6, X3/X4

/// X1: el score o el cierre de la sesión de otra persona, de una inexistente o con un id mal
/// formado → el mismo 404, y no se toca nada.
#[tokio::test]
async fn x1_sesiones_ajenas_dan_404() {
    let w = mundo().await;
    let otra = registrar(&w.app, "otra.persona@correo-prueba.com").await;
    let (ap_otra, ajena) = sesion_nueva(&w.db, &w.app, &otra, "ana", "junior").await;
    sembrar_respuestas(&w.db, ajena, 2).await;
    for id in [
        ajena.to_string(),
        Uuid::now_v7().to_string(),
        "no-es-uuid".to_string(),
    ] {
        for (metodo, uri) in [("POST", url_finish(&id)), ("GET", url_score(&id))] {
            let r = pedir(&w.app, metodo, &uri, Some(&w.token), None).await;
            assert_eq!(r.estado, StatusCode::NOT_FOUND, "{metodo} {uri}");
            assert_eq!(error_de(&r.cuerpo), "no_encontrado");
        }
    }
    assert_eq!(estado_de(&w.db, ajena).await, "in_progress");
    assert_eq!(n_progreso(&w.db, ap_otra).await, 0);
    // Ni siquiera completada se ve el score ajeno.
    assert_eq!(
        pedir(&w.app, "POST", &url_finish(ajena), Some(&otra), None)
            .await
            .estado,
        StatusCode::OK
    );
    let r = pedir(&w.app, "GET", &url_score(ajena), Some(&w.token), None).await;
    assert_eq!(r.estado, StatusCode::NOT_FOUND);
    w.db.cleanup().await;
}

/// X2: sin token (o con uno inválido) → 401.
#[tokio::test]
async fn x2_sin_token_es_401() {
    let w = mundo().await;
    for token in [None, Some("basura")] {
        for (metodo, uri) in [("POST", url_finish(w.sesion)), ("GET", url_score(w.sesion))] {
            let r = pedir(&w.app, metodo, &uri, token, None).await;
            assert_eq!(r.estado, StatusCode::UNAUTHORIZED, "{metodo} {uri}");
        }
    }
    w.db.cleanup().await;
}

/// X6: una cuenta suspendida recibe 403 `cuenta_no_disponible` y no se toca nada.
#[tokio::test]
async fn x6_una_cuenta_suspendida_recibe_403() {
    let w = mundo().await;
    sembrar_respuestas(&w.db, w.sesion, 2).await;
    let user = jwt().validate(&w.token).unwrap();
    w.db.exec(&format!(
        "UPDATE users SET status = 'suspended' WHERE id = '{user}'"
    ))
    .await
    .unwrap();
    for (metodo, uri) in [("POST", url_finish(w.sesion)), ("GET", url_score(w.sesion))] {
        let r = pedir(&w.app, metodo, &uri, Some(&w.token), None).await;
        assert_eq!(r.estado, StatusCode::FORBIDDEN, "{metodo}");
        assert_eq!(error_de(&r.cuerpo), "cuenta_no_disponible");
    }
    assert_eq!(estado_de(&w.db, w.sesion).await, "in_progress");
    assert_eq!(n_scores_total(&w.db).await, 0);
    w.db.cleanup().await;
}

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Buffer {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capturar_logs() -> (Buffer, tracing::subscriber::DefaultGuard) {
    let buffer = Buffer::default();
    let escritor = buffer.clone();
    let capa = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(move || escritor.clone());
    let suscriptor = tracing_subscriber::registry()
        .with(jobout_core_api::telemetry::env_filter(Some("trace")))
        .with(capa);
    (buffer, tracing::subscriber::set_default(suscriptor))
}

fn log_de(b: &Buffer) -> String {
    String::from_utf8_lossy(&b.0.lock().unwrap()).into_owned()
}

/// X3/X4 (sanidad): la captura funciona. Sin esto, «no aparece en el log» no probaría nada.
#[tokio::test]
async fn x4a_la_captura_de_logs_funciona() {
    let (buffer, _guarda) = capturar_logs();
    tracing::error!(
        donde = "prueba_de_captura",
        detalle = MARCA,
        "evento de prueba"
    );
    assert!(log_de(&buffer).contains(MARCA));
}

/// X3/X4: ni el texto de la persona, ni el del recruiter, ni el «por qué» del desglose salen en el
/// log ni en el cuerpo de un error: con un cierre correcto, un 422, un 409, un 503 y un 500.
#[tokio::test]
async fn x4_ni_el_texto_ni_el_desglose_salen_en_logs_ni_en_errores() {
    let (buffer, _guarda) = capturar_logs();
    let w = mundo().await;
    let mut errores: Vec<Value> = vec![];

    // 422 (corta) con la marca ya guardada.
    sembrar_respuestas_con(&w.db, w.sesion, 1, &format!("{MARCA} {BUENA}")).await;
    sembrar_turno(
        &w.db,
        w.sesion,
        4,
        "recruiter",
        "follow_up",
        &format!("{MARCA}-recruiter"),
    )
    .await;
    let r = terminar(&w).await;
    assert_eq!(r.estado, StatusCode::UNPROCESSABLE_ENTITY);
    errores.push(r.cuerpo);

    // 503 (el motor no evalúa).
    sembrar_respuestas_con(&w.db, w.sesion, 1, &format!("{MARCA} {BUENA}")).await;
    let caido = app_con(&w.db, Arc::new(MotorSinEvaluar));
    let r = pedir(&caido, "POST", &url_finish(w.sesion), Some(&w.token), None).await;
    assert_eq!(r.estado, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        log_de(&buffer).contains("motor"),
        "el fallo del motor se registra por categoría"
    );
    errores.push(r.cuerpo);

    // 409 (el score de una sesión abierta).
    let r = score_de(&w).await;
    assert_eq!(r.estado, StatusCode::CONFLICT);
    errores.push(r.cuerpo);

    // 500 (un turno que no se descifra).
    let mut mal = encrypt_turn_text(&common::llaves(), "x").unwrap();
    mal[20] ^= 1;
    w.db.exec(&format!(
        "UPDATE interview_turns SET transcript = '\\x{}'::bytea WHERE interview_session_id = '{}' AND seq = 2",
        common::hex(&mal),
        w.sesion
    ))
    .await
    .unwrap();
    let r = terminar(&w).await;
    assert_eq!(r.estado, StatusCode::INTERNAL_SERVER_ERROR, "{}", r.cuerpo);
    assert_eq!(error_de(&r.cuerpo), "error_interno");
    errores.push(r.cuerpo);
    // Se restaura el turno.
    let bueno = encrypt_turn_text(&common::llaves(), &format!("{MARCA} {BUENA}")).unwrap();
    w.db.exec(&format!(
        "UPDATE interview_turns SET transcript = '\\x{}'::bytea WHERE interview_session_id = '{}' AND seq = 2",
        common::hex(&bueno),
        w.sesion
    ))
    .await
    .unwrap();

    // Correcto, y la lectura del score.
    let ok = terminar(&w).await;
    assert_eq!(ok.estado, StatusCode::OK, "{}", ok.cuerpo);
    let g = score_de(&w).await;
    assert_eq!(g.estado, StatusCode::OK);

    for c in &errores {
        let t = c.to_string();
        assert!(!t.contains(MARCA), "se coló en un error: {t}");
    }
    let log = log_de(&buffer);
    assert!(log.contains("error interno"), "{log:?}");
    assert!(!log.contains(MARCA), "el texto salió en el log: {log}");
    assert!(!log.contains("Rust"), "el texto salió en el log");
    for d in ok.cuerpo["breakdown"].as_array().unwrap() {
        let why = d["why"].as_str().unwrap();
        assert!(!log.contains(why), "el «por qué» salió en el log: {why}");
    }
    for m in ok.cuerpo["improvements"].as_array().unwrap() {
        assert!(
            !log.contains(m.as_str().unwrap()),
            "una mejora salió en el log"
        );
    }
    w.db.cleanup().await;
}

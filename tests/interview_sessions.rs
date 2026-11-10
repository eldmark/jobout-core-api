//! Casos de verdad de las sesiones de entrevista (paso 6 de docs/working/fase3-entrevista.md):
//! S1–S14 y, para estas rutas, X1, X2, X3, X5 y X6. Contra PostgreSQL real; una base por prueba.
//!
//! Se escribieron antes que la implementación, en rojo. Ver CONTRIBUTING.md.

#[allow(dead_code)]
mod common;

use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
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
        store_crypto::encrypt_turn_text, DeterministicEngine, InterviewEngine, Level,
        RecruiterCode, RecruiterConfig,
    },
    interview_store::{self, CreateOutcome},
    jwt::JwtKeys,
    state::AppState,
};
use sea_orm::{ConnectionTrait, Statement};
use serde_json::{json, Value};
use tower::ServiceExt;
use tracing_subscriber::layer::SubscriberExt;
use uuid::Uuid;

const CLAVE: &str = "una-clave-larga-123";

fn pii() -> Arc<PiiKeys> {
    Arc::new(
        PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
    )
}

fn jwt() -> Arc<JwtKeys> {
    Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap())
}

fn app(db: &TestDb) -> Router {
    app_with_state(AppState {
        db: db.conn.clone(),
        pii: pii(),
        jwt: jwt(),
        blobs: jobout_core_api::blob::memory_store(),
    })
}

async fn enviar(
    app: &Router,
    metodo: &str,
    uri: &str,
    token: Option<&str>,
    cuerpo: Option<Value>,
) -> (StatusCode, Value) {
    let mut peticion = Request::builder().method(metodo).uri(uri);
    if let Some(t) = token {
        peticion = peticion.header("authorization", format!("Bearer {t}"));
    }
    let cuerpo = match cuerpo {
        Some(json) => {
            peticion = peticion.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let respuesta = app
        .clone()
        .oneshot(peticion.body(cuerpo).unwrap())
        .await
        .unwrap();
    let estado = respuesta.status();
    let bytes = respuesta.into_body().collect().await.unwrap().to_bytes();
    (
        estado,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Igual que `enviar` pero con el cuerpo en crudo (para JSON roto).
async fn enviar_crudo(app: &Router, uri: &str, token: &str, crudo: &str) -> (StatusCode, Value) {
    let peticion = Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(crudo.to_string()))
        .unwrap();
    let r = app.clone().oneshot(peticion).await.unwrap();
    let estado = r.status();
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    (
        estado,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn registrar(app: &Router, email: &str) -> String {
    let (estado, cuerpo) = enviar(
        app,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(
            json!({"email": email, "password": CLAVE, "first_name": "Ana", "last_name": "Prueba"}),
        ),
    )
    .await;
    assert_eq!(estado, StatusCode::CREATED, "{cuerpo}");
    cuerpo["access_token"].as_str().unwrap().to_string()
}

struct Mundo {
    db: TestDb,
    app: Router,
    token: String,
    applicant: Uuid,
}

async fn mundo() -> Mundo {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let token = registrar(&app, "ana.uno@correo-prueba.com").await;
    let applicant = applicant_de(&db, &token).await;
    Mundo {
        db,
        app,
        token,
        applicant,
    }
}

async fn applicant_de(db: &TestDb, token: &str) -> Uuid {
    let user = jwt().validate(token).unwrap();
    common::uuid_de(
        db,
        &format!("SELECT id FROM applicants WHERE user_id = '{user}'"),
    )
    .await
}

async fn id_recruiter(db: &TestDb, code: &str) -> Uuid {
    common::uuid_de(
        db,
        &format!("SELECT id FROM ai_recruiters WHERE code = '{code}'"),
    )
    .await
}

async fn id_nivel(db: &TestDb, code: &str) -> Uuid {
    common::uuid_de(
        db,
        &format!("SELECT id FROM interview_levels WHERE code = '{code}'"),
    )
    .await
}

async fn crear(w: &Mundo, recruiter: &str, nivel: &str) -> (StatusCode, Value) {
    crear_con(&w.app, &w.token, &w.db, recruiter, nivel).await
}

async fn crear_con(
    app: &Router,
    token: &str,
    db: &TestDb,
    recruiter: &str,
    nivel: &str,
) -> (StatusCode, Value) {
    let r = id_recruiter(db, recruiter).await;
    let n = id_nivel(db, nivel).await;
    enviar(
        app,
        "POST",
        "/api/v1/interviews",
        Some(token),
        Some(json!({"recruiter_id": r, "level_id": n})),
    )
    .await
}

async fn ruta(
    w: &Mundo,
    metodo: &str,
    id: impl std::fmt::Display,
    sufijo: &str,
) -> (StatusCode, Value) {
    enviar(
        &w.app,
        metodo,
        &format!("/api/v1/interviews/{id}{sufijo}"),
        Some(&w.token),
        None,
    )
    .await
}

/// Inserta una sesión directamente. `creada` es una expresión SQL (`now() - interval '2 days'`).
async fn sembrar(
    db: &TestDb,
    applicant: Uuid,
    recruiter: &str,
    nivel: &str,
    estado: &str,
    creada: &str,
) -> Uuid {
    let id = Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO interview_sessions (id, applicant_id, ai_recruiter_id, interview_level_id, \
            rubric_version_id, status, started_at, paused_at, completed_at, created_at) \
         SELECT '{id}', '{applicant}', r.id, l.id, \
            (SELECT v.id FROM rubric_versions v WHERE v.rubric_id = r.rubric_id AND v.is_current), \
            '{estado}'::interview_status, {creada}, \
            CASE WHEN '{estado}' = 'paused' THEN now() END, \
            CASE WHEN '{estado}' = 'completed' THEN {creada} END, {creada} \
         FROM ai_recruiters r, interview_levels l WHERE r.code = '{recruiter}' AND l.code = '{nivel}'"
    ))
    .await
    .unwrap();
    id
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

async fn estado_de(db: &TestDb, id: Uuid) -> String {
    let fila = db
        .conn
        .query_one(Statement::from_string(
            db.conn.get_database_backend(),
            format!("SELECT status::text FROM interview_sessions WHERE id = '{id}'"),
        ))
        .await
        .unwrap()
        .expect("sesión");
    fila.try_get_by_index::<String>(0).unwrap()
}

fn primera_pregunta(recruiter: RecruiterCode, nivel: Level) -> String {
    DeterministicEngine
        .next_turn(
            &RecruiterConfig {
                recruiter,
                level: nivel,
            },
            &[],
        )
        .unwrap()
        .message
}

fn error_de(cuerpo: &Value) -> &str {
    cuerpo["error"].as_str().unwrap_or("")
}

// ================================================================== S1, S2

/// S1: crear → 201 con la forma del contrato y la primera pregunta del motor.
#[tokio::test]
async fn s1_crear_devuelve_la_sesion_con_la_primera_pregunta() {
    let w = mundo().await;
    let (estado, c) = crear(&w, "ana", "junior").await;
    assert_eq!(estado, StatusCode::CREATED, "{c}");

    assert!(Uuid::parse_str(c["id"].as_str().unwrap()).is_ok());
    assert_eq!(c["status"], "in_progress");
    assert_eq!(
        c["recruiter"]["id"],
        id_recruiter(&w.db, "ana").await.to_string()
    );
    assert_eq!(c["recruiter"]["display_name"], "Ana");
    assert_eq!(c["recruiter"]["role_title"], "RRHH");
    assert_eq!(
        c["level"]["id"],
        id_nivel(&w.db, "junior").await.to_string()
    );
    assert_eq!(c["level"]["code"], "junior");
    assert_eq!(c["level"]["name"], "Junior");
    assert!(c["created_at"].as_str().unwrap().ends_with('Z'), "{c}");
    let turnos = c["turns"].as_array().unwrap();
    assert_eq!(turnos.len(), 1, "solo la primera pregunta");
    assert_eq!(turnos[0]["seq"], 1);
    assert_eq!(turnos[0]["speaker"], "recruiter");
    assert_eq!(turnos[0]["kind"], "question");
    assert_eq!(
        turnos[0]["text"],
        primera_pregunta(RecruiterCode::Ana, Level::Junior)
    );

    // En la base: en curso, con inicio, y un solo turno.
    let id: Uuid = c["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(estado_de(&w.db, id).await, "in_progress");
    assert_eq!(
        w.db.count(&format!(
            "SELECT count(*) FROM interview_sessions WHERE id = '{id}' AND started_at IS NOT NULL \
             AND applicant_id = '{}'",
            w.applicant
        ))
        .await,
        1
    );
    assert_eq!(
        w.db.count(&format!(
            "SELECT count(*) FROM interview_turns WHERE interview_session_id = '{id}'"
        ))
        .await,
        1
    );
    w.db.cleanup().await;
}

/// S1 (cifrado): el texto del turno guardado es SIEMPRE cifrado y se descifra con la clave.
#[tokio::test]
async fn s1b_el_texto_del_turno_se_guarda_cifrado() {
    let w = mundo().await;
    let (_, c) = crear(&w, "martin", "junior").await;
    let id = c["id"].as_str().unwrap();
    let texto = c["turns"][0]["text"].as_str().unwrap().to_string();
    let guardado = common::bytes_de(
        &w.db,
        &format!("SELECT transcript FROM interview_turns WHERE interview_session_id = '{id}'"),
    )
    .await;
    assert!(
        !common::contiene(&guardado, texto.as_bytes()),
        "texto en claro en la base"
    );
    assert!(!common::contiene(&guardado, &texto.as_bytes()[..12]));
    assert_eq!(
        jobout_core_api::interview::store_crypto::decrypt_turn_text(&common::llaves(), &guardado)
            .unwrap(),
        texto
    );
    w.db.cleanup().await;
}

/// S1: cada recruiter abre con su propia pregunta (la del motor, no una fija).
#[tokio::test]
async fn s1c_la_apertura_es_la_del_recruiter_elegido() {
    let w = mundo().await;
    let (_, a) = crear(&w, "ana", "junior").await;
    ruta(&w, "POST", a["id"].as_str().unwrap(), "/abandon").await;
    let (e, s) = crear(&w, "sofia", "junior").await;
    assert_eq!(e, StatusCode::CREATED, "{s}");
    assert_eq!(
        s["turns"][0]["text"],
        primera_pregunta(RecruiterCode::Sofia, Level::Junior)
    );
    assert_ne!(a["turns"][0]["text"], s["turns"][0]["text"]);
    w.db.cleanup().await;
}

/// S2: la sesión fija la versión vigente al empezar; publicar otra después no la cambia.
#[tokio::test]
async fn s2_fija_la_version_de_la_rubrica_vigente_al_empezar() {
    let w = mundo().await;
    let v1 = common::uuid_de(
        &w.db,
        "SELECT v.id FROM rubric_versions v JOIN ai_recruiters r ON r.rubric_id = v.rubric_id \
         WHERE r.code = 'ana' AND v.is_current",
    )
    .await;
    let (_, c) = crear(&w, "ana", "junior").await;
    let id: Uuid = c["id"].as_str().unwrap().parse().unwrap();
    let fijada =
        |id: Uuid| format!("SELECT rubric_version_id FROM interview_sessions WHERE id = '{id}'");
    assert_eq!(common::uuid_de(&w.db, &fijada(id)).await, v1);

    // Se publica la versión 2 y pasa a ser la vigente.
    w.db.exec(
        "UPDATE rubric_versions SET is_current = false WHERE id IN \
           (SELECT v.id FROM rubric_versions v JOIN ai_recruiters r ON r.rubric_id = v.rubric_id \
            WHERE r.code = 'ana' AND v.is_current)",
    )
    .await
    .unwrap();
    w.db.exec(
        "INSERT INTO rubric_versions (id, rubric_id, version, criteria, published_at, is_current) \
         SELECT gen_random_uuid(), r.rubric_id, 2, \
                (SELECT criteria FROM rubric_versions WHERE rubric_id = r.rubric_id AND version = 1), \
                now(), true FROM ai_recruiters r WHERE r.code = 'ana'",
    )
    .await
    .unwrap();
    assert_eq!(
        common::uuid_de(&w.db, &fijada(id)).await,
        v1,
        "la sesión no cambia"
    );

    // Una sesión nueva sí toma la vigente de ahora.
    ruta(&w, "POST", id, "/abandon").await;
    let (_, d) = crear(&w, "ana", "junior").await;
    let nueva: Uuid = d["id"].as_str().unwrap().parse().unwrap();
    assert_ne!(common::uuid_de(&w.db, &fijada(nueva)).await, v1);
    w.db.cleanup().await;
}

// ================================================================== S3, S4

/// S3: nivel no disponible → 409; nivel disponible pero bloqueado → 403; con progreso → 201.
#[tokio::test]
async fn s3_nivel_bloqueado_y_no_disponible() {
    let w = mundo().await;
    let (e, c) = crear(&w, "ana", "startup").await;
    assert_eq!(e, StatusCode::FORBIDDEN, "{c}");
    assert_eq!(error_de(&c), "nivel_bloqueado");
    assert!(c["message"].as_str().is_some_and(|m| !m.is_empty()));

    for nivel in ["amazon", "google", "mckinsey"] {
        let (e, c) = crear(&w, "ana", nivel).await;
        assert_eq!(e, StatusCode::CONFLICT, "{nivel}: {c}");
        assert_eq!(error_de(&c), "nivel_no_disponible", "{nivel}");
    }
    assert_eq!(
        w.db.count("SELECT count(*) FROM interview_sessions").await,
        0
    );

    // Con una fila de progreso con `unlocked_at`, Startup se abre.
    w.db.exec(&format!(
        "INSERT INTO applicant_level_progress (id, applicant_id, interview_level_id, unlocked_at) \
         VALUES (gen_random_uuid(), '{}', '{}', now())",
        w.applicant,
        id_nivel(&w.db, "startup").await
    ))
    .await
    .unwrap();
    let (e, c) = crear(&w, "ana", "startup").await;
    assert_eq!(e, StatusCode::CREATED, "{c}");
    assert_eq!(c["level"]["code"], "startup");
    w.db.cleanup().await;
}

/// S3: una fila de progreso de OTRA persona no desbloquea el nivel para mí.
#[tokio::test]
async fn s3b_el_desbloqueo_de_otra_persona_no_cuenta() {
    let w = mundo().await;
    let (_, otro) = common::nuevo_applicant(&w.db, "otro").await;
    w.db.exec(&format!(
        "INSERT INTO applicant_level_progress (id, applicant_id, interview_level_id, unlocked_at) \
         VALUES (gen_random_uuid(), '{otro}', '{}', now())",
        id_nivel(&w.db, "startup").await
    ))
    .await
    .unwrap();
    let (e, c) = crear(&w, "ana", "startup").await;
    assert_eq!(e, StatusCode::FORBIDDEN, "{c}");
    w.db.cleanup().await;
}

/// S4: inexistentes → 404; ids mal formados en el CUERPO → 422 (contrato); en el path → 404.
#[tokio::test]
async fn s4_ids_inexistentes_y_mal_formados() {
    let w = mundo().await;
    let r = id_recruiter(&w.db, "ana").await;
    let n = id_nivel(&w.db, "junior").await;
    let nada = Uuid::now_v7();

    for cuerpo in [
        json!({"recruiter_id": nada, "level_id": n}),
        json!({"recruiter_id": r, "level_id": nada}),
    ] {
        let (e, c) = enviar(
            &w.app,
            "POST",
            "/api/v1/interviews",
            Some(&w.token),
            Some(cuerpo),
        )
        .await;
        assert_eq!(e, StatusCode::NOT_FOUND, "{c}");
        assert_eq!(error_de(&c), "no_encontrado");
    }

    for cuerpo in [
        json!({"recruiter_id": "no-es-uuid", "level_id": n}),
        json!({"recruiter_id": r, "level_id": "123"}),
        json!({"recruiter_id": r}),
        json!({"level_id": n}),
        json!({}),
        json!({"recruiter_id": 5, "level_id": n}),
        json!({"recruiter_id": null, "level_id": null}),
        json!([]),
    ] {
        let (e, c) = enviar(
            &w.app,
            "POST",
            "/api/v1/interviews",
            Some(&w.token),
            Some(cuerpo.clone()),
        )
        .await;
        assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{cuerpo}: {c}");
        assert_eq!(error_de(&c), "datos_invalidos");
    }
    for crudo in ["", "{", "no es json"] {
        let (e, c) = enviar_crudo(&w.app, "/api/v1/interviews", &w.token, crudo).await;
        assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{crudo:?}: {c}");
        assert_eq!(error_de(&c), "datos_invalidos");
    }
    // Sin cuerpo ni content-type.
    let (e, c) = enviar(&w.app, "POST", "/api/v1/interviews", Some(&w.token), None).await;
    assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{c}");

    // En el path, un id que no es UUID es 404 en todas las rutas.
    for (m, s) in [
        ("GET", ""),
        ("DELETE", ""),
        ("POST", "/pause"),
        ("POST", "/resume"),
        ("POST", "/abandon"),
    ] {
        let (e, c) = ruta(&w, m, "no-es-uuid", s).await;
        assert_eq!(e, StatusCode::NOT_FOUND, "{m} {s}");
        assert_eq!(error_de(&c), "no_encontrado");
    }
    assert_eq!(
        w.db.count("SELECT count(*) FROM interview_sessions").await,
        0
    );
    w.db.cleanup().await;
}

/// Orden de validación del contrato: cuerpo → existen → disponible → desbloqueado → sin sesión
/// activa. Cada paso gana al siguiente aunque el siguiente también fallaría.
#[tokio::test]
async fn orden_de_validacion_de_crear() {
    let w = mundo().await;
    let (e, _) = crear(&w, "ana", "junior").await;
    assert_eq!(e, StatusCode::CREATED); // ahora hay una sesión activa

    let r = id_recruiter(&w.db, "ana").await;
    let nada = Uuid::now_v7();
    // inexistente gana a la sesión activa
    let (e, c) = enviar(
        &w.app,
        "POST",
        "/api/v1/interviews",
        Some(&w.token),
        Some(json!({"recruiter_id": r, "level_id": nada})),
    )
    .await;
    assert_eq!((e, error_de(&c)), (StatusCode::NOT_FOUND, "no_encontrado"));
    // no disponible gana a la sesión activa (y al bloqueo)
    let (e, c) = crear(&w, "ana", "amazon").await;
    assert_eq!(
        (e, error_de(&c)),
        (StatusCode::CONFLICT, "nivel_no_disponible")
    );
    // bloqueado gana a la sesión activa
    let (e, c) = crear(&w, "ana", "startup").await;
    assert_eq!(
        (e, error_de(&c)),
        (StatusCode::FORBIDDEN, "nivel_bloqueado")
    );
    // cuerpo inválido gana a todo
    let (e, c) = enviar(
        &w.app,
        "POST",
        "/api/v1/interviews",
        Some(&w.token),
        Some(json!({"recruiter_id": "x", "level_id": nada})),
    )
    .await;
    assert_eq!(
        (e, error_de(&c)),
        (StatusCode::UNPROCESSABLE_ENTITY, "datos_invalidos")
    );
    w.db.cleanup().await;
}

// ================================================================== S5, S6

/// S5: con una sesión activa (en curso o pausada), crear otra → 409 con el id de la activa.
#[tokio::test]
async fn s5_con_sesion_activa_no_se_crea_otra() {
    let w = mundo().await;
    let (_, c) = crear(&w, "ana", "junior").await;
    let activa = c["id"].as_str().unwrap().to_string();

    let (e, d) = crear(&w, "martin", "junior").await;
    assert_eq!(e, StatusCode::CONFLICT, "{d}");
    assert_eq!(error_de(&d), "sesion_en_curso");
    assert_eq!(d["session_id"], activa);
    assert!(d["message"].as_str().is_some_and(|m| !m.is_empty()));

    // Pausada también cuenta como activa.
    ruta(&w, "POST", &activa, "/pause").await;
    let (e, d) = crear(&w, "martin", "junior").await;
    assert_eq!(e, StatusCode::CONFLICT, "{d}");
    assert_eq!(d["session_id"], activa);
    assert_eq!(
        w.db.count("SELECT count(*) FROM interview_sessions").await,
        1,
        "no se creó nada"
    );
    // La sesión activa de OTRA persona no me estorba.
    let otro = registrar(&w.app, "otra.persona@correo-prueba.com").await;
    let (e, d) = crear_con(&w.app, &otro, &w.db, "ana", "junior").await;
    assert_eq!(e, StatusCode::CREATED, "{d}");
    w.db.cleanup().await;
}

/// S6: muchas creaciones simultáneas de la misma persona dejan UNA sola sesión activa; las
/// demás reciben `sesion_en_curso` con el id de la ganadora. Se repite varias rondas.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s6_creaciones_simultaneas_dejan_una_sola_sesion() {
    let w = mundo().await;
    let r = id_recruiter(&w.db, "ana").await;
    let n = id_nivel(&w.db, "junior").await;

    for ronda in 0..5 {
        let barrera = Arc::new(tokio::sync::Barrier::new(8));
        let mut tareas = Vec::new();
        for _ in 0..8 {
            let (app, token, barrera) = (w.app.clone(), w.token.clone(), barrera.clone());
            tareas.push(tokio::spawn(async move {
                barrera.wait().await;
                enviar(
                    &app,
                    "POST",
                    "/api/v1/interviews",
                    Some(&token),
                    Some(json!({"recruiter_id": r, "level_id": n})),
                )
                .await
            }));
        }
        let mut creadas = Vec::new();
        let mut rechazadas = Vec::new();
        for t in tareas {
            let (e, c) = t.await.unwrap();
            match e {
                StatusCode::CREATED => creadas.push(c),
                StatusCode::CONFLICT => rechazadas.push(c),
                otro => panic!("ronda {ronda}: estado inesperado {otro}: {c}"),
            }
        }
        assert_eq!(
            creadas.len(),
            1,
            "ronda {ronda}: debe ganar exactamente una"
        );
        let ganadora = creadas[0]["id"].as_str().unwrap();
        assert_eq!(rechazadas.len(), 7);
        for c in &rechazadas {
            assert_eq!(error_de(c), "sesion_en_curso", "{c}");
            assert_eq!(c["session_id"], ganadora);
        }
        assert_eq!(
            w.db.count(
                "SELECT count(*) FROM interview_sessions WHERE status IN ('in_progress','paused')"
            )
            .await,
            1
        );
        // La ganadora tiene exactamente su primera pregunta; las perdedoras no dejaron turnos.
        assert_eq!(
            w.db.count("SELECT count(*) FROM interview_turns").await,
            ronda as i64 + 1
        );
        ruta(&w, "POST", ganadora, "/abandon").await;
    }
    w.db.cleanup().await;
}

/// S6 (capa de datos): la violación del índice parcial se traduce a `OpenExists` con el id de la
/// sesión abierta, sin dejar nada a medias (ni sesión ni turno).
#[tokio::test]
async fn s6b_la_violacion_del_indice_se_traduce_a_sesion_abierta() {
    let w = mundo().await;
    let abierta = sembrar(&w.db, w.applicant, "ana", "junior", "paused", "now()").await;
    let r = id_recruiter(&w.db, "ana").await;
    let n = id_nivel(&w.db, "junior").await;

    let resultado =
        interview_store::create_session(&w.db.conn, &pii(), w.applicant, r, n, "¿Quién eres?")
            .await
            .expect("no debe ser un error");
    match resultado {
        CreateOutcome::OpenExists { session_id } => assert_eq!(session_id, abierta),
        CreateOutcome::Created { .. } => panic!("se creó una segunda sesión abierta"),
    }
    assert_eq!(
        w.db.count("SELECT count(*) FROM interview_sessions").await,
        1
    );
    assert_eq!(w.db.count("SELECT count(*) FROM interview_turns").await, 0);
    w.db.cleanup().await;
}

// ================================================================== S7

fn nombres(items: &Value) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect()
}

/// S7: el listado es mío, trae estado, nivel, recruiter y score si lo hay, y no trae transcripciones.
#[tokio::test]
async fn s7_el_listado_es_mio_y_sin_transcripciones() {
    let w = mundo().await;
    let completa = sembrar(
        &w.db,
        w.applicant,
        "ana",
        "junior",
        "completed",
        "now() - interval '2 days'",
    )
    .await;
    sembrar_turno(
        &w.db,
        completa,
        1,
        "recruiter",
        "question",
        "MARCA-PREGUNTA",
    )
    .await;
    sembrar_turno(&w.db, completa, 2, "applicant", "answer", "MARCA-RESPUESTA").await;
    w.db.exec(&format!(
        "INSERT INTO interview_scores (id, interview_session_id, score, clarity, breakdown, rubric_version_id) \
         SELECT gen_random_uuid(), id, 72, 65, '[]'::jsonb, rubric_version_id \
         FROM interview_sessions WHERE id = '{completa}'"
    ))
    .await
    .unwrap();
    let abierta = sembrar(
        &w.db,
        w.applicant,
        "sofia",
        "junior",
        "in_progress",
        "now() - interval '1 day'",
    )
    .await;

    // La de otra persona no aparece.
    let (_, otro) = common::nuevo_applicant(&w.db, "otra").await;
    sembrar(&w.db, otro, "ana", "junior", "completed", "now()").await;

    let (e, c) = enviar(&w.app, "GET", "/api/v1/interviews", Some(&w.token), None).await;
    assert_eq!(e, StatusCode::OK, "{c}");
    assert_eq!(
        nombres(&c["items"]),
        vec![abierta.to_string(), completa.to_string()]
    );
    assert!(c["next_cursor"].is_null());

    let a = &c["items"][0];
    assert_eq!(a["status"], "in_progress");
    assert_eq!(a["level"]["code"], "junior");
    assert_eq!(a["level"]["name"], "Junior");
    assert_eq!(a["recruiter"]["display_name"], "Sofía");
    assert!(a["score"].is_null());
    assert!(a["completed_at"].is_null());
    assert!(a["created_at"].as_str().unwrap().ends_with('Z'));

    let b = &c["items"][1];
    assert_eq!(b["status"], "completed");
    assert_eq!(b["score"], 72);
    assert!(b["completed_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(b["recruiter"]["display_name"], "Ana");

    // Sin transcripción: ni campo `turns` ni texto de ningún turno.
    let plano = c.to_string();
    assert!(!plano.contains("MARCA-"), "{plano}");
    for i in c["items"].as_array().unwrap() {
        assert!(i.get("turns").is_none());
        assert!(i["recruiter"].get("role_title").is_none());
    }
    w.db.cleanup().await;
}

/// S7: paginación por cursor, más recientes primero, sin duplicados ni huecos, incluso con
/// sesiones con el mismo `created_at`.
#[tokio::test]
async fn s7b_paginacion_por_cursor() {
    let w = mundo().await;
    let mut esperado = Vec::new();
    for dias in (1..=3).rev() {
        esperado.push(
            sembrar(
                &w.db,
                w.applicant,
                "ana",
                "junior",
                "abandoned",
                &format!("now() - interval '{dias} days'"),
            )
            .await,
        );
    }
    // Tres con exactamente el mismo instante.
    for _ in 0..3 {
        esperado.push(
            sembrar(
                &w.db,
                w.applicant,
                "ana",
                "junior",
                "completed",
                "timestamptz '2026-01-01 10:00:00+00'",
            )
            .await,
        );
    }
    // Orden esperado: created_at desc, id desc.
    let mut con_fecha: Vec<(String, String)> = Vec::new();
    for id in &esperado {
        let fila = w
            .db
            .conn
            .query_one(Statement::from_string(
                w.db.conn.get_database_backend(),
                format!("SELECT to_char(created_at AT TIME ZONE 'UTC','YYYYMMDDHH24MISSUS'), id::text FROM interview_sessions WHERE id = '{id}'"),
            ))
            .await
            .unwrap()
            .unwrap();
        con_fecha.push((
            fila.try_get_by_index(0).unwrap(),
            fila.try_get_by_index(1).unwrap(),
        ));
    }
    con_fecha.sort();
    con_fecha.reverse();
    let orden: Vec<String> = con_fecha.into_iter().map(|(_, id)| id).collect();

    let mut vistos = Vec::new();
    let mut cursor: Option<String> = None;
    let mut paginas = 0;
    loop {
        let mut uri = "/api/v1/interviews?limit=2".to_string();
        if let Some(c) = &cursor {
            uri.push_str(&format!("&cursor={c}"));
        }
        let (e, c) = enviar(&w.app, "GET", &uri, Some(&w.token), None).await;
        assert_eq!(e, StatusCode::OK, "{c}");
        assert!(c["items"].as_array().unwrap().len() <= 2);
        vistos.extend(nombres(&c["items"]));
        paginas += 1;
        match c["next_cursor"].as_str() {
            Some(sig) => cursor = Some(sig.to_string()),
            None => break,
        }
        assert!(paginas < 10, "el cursor no avanza");
    }
    assert_eq!(paginas, 3);
    assert_eq!(vistos, orden);

    // Una última página exacta: limit = total → next_cursor null.
    let (_, c) = enviar(
        &w.app,
        "GET",
        "/api/v1/interviews?limit=6",
        Some(&w.token),
        None,
    )
    .await;
    assert!(c["next_cursor"].is_null());
    assert_eq!(c["items"].as_array().unwrap().len(), 6);
    // Defecto 20 y tope 50.
    let (e, _) = enviar(
        &w.app,
        "GET",
        "/api/v1/interviews?limit=50",
        Some(&w.token),
        None,
    )
    .await;
    assert_eq!(e, StatusCode::OK);
    w.db.cleanup().await;
}

/// S7: filtro por estado y entradas inválidas → 422.
#[tokio::test]
async fn s7c_filtro_por_estado_y_parametros_invalidos() {
    let w = mundo().await;
    sembrar(
        &w.db,
        w.applicant,
        "ana",
        "junior",
        "completed",
        "now() - interval '3 days'",
    )
    .await;
    sembrar(
        &w.db,
        w.applicant,
        "ana",
        "junior",
        "abandoned",
        "now() - interval '2 days'",
    )
    .await;
    let pausada = sembrar(
        &w.db,
        w.applicant,
        "ana",
        "junior",
        "paused",
        "now() - interval '1 days'",
    )
    .await;

    let (_, c) = enviar(
        &w.app,
        "GET",
        "/api/v1/interviews?status=paused",
        Some(&w.token),
        None,
    )
    .await;
    assert_eq!(nombres(&c["items"]), vec![pausada.to_string()]);
    for s in ["in_progress", "paused", "completed", "abandoned"] {
        let (e, _) = enviar(
            &w.app,
            "GET",
            &format!("/api/v1/interviews?status={s}"),
            Some(&w.token),
            None,
        )
        .await;
        assert_eq!(e, StatusCode::OK, "{s}");
    }

    // Un cursor válido (de una página real) para probar que los otros fallan por sí mismos.
    for malo in [
        "limit=0",
        "limit=51",
        "limit=-1",
        "limit=abc",
        "limit=",
        "limit=1.5",
        "status=created",
        "status=otra",
        "status=",
        "status=PAUSED",
        "cursor=",
        "cursor=basura",
        "cursor=!!!",
        "cursor=MDowOjA", // base64 válido pero no es un cursor
    ] {
        let (e, c) = enviar(
            &w.app,
            "GET",
            &format!("/api/v1/interviews?{malo}"),
            Some(&w.token),
            None,
        )
        .await;
        assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{malo}: {c}");
        assert_eq!(error_de(&c), "datos_invalidos", "{malo}");
    }
    w.db.cleanup().await;
}

// ================================================================== S8

/// S8: GET por id devuelve la sesión con todos los turnos descifrados y en orden de `seq`.
#[tokio::test]
async fn s8_leer_una_sesion_con_todos_los_turnos_descifrados() {
    let w = mundo().await;
    let (_, c) = crear(&w, "ana", "junior").await;
    let id = c["id"].as_str().unwrap().to_string();
    let uuid: Uuid = id.parse().unwrap();
    // Se insertan fuera de orden a propósito.
    sembrar_turno(
        &w.db,
        uuid,
        3,
        "recruiter",
        "follow_up",
        "Tercero, con ñ y 🚀",
    )
    .await;
    sembrar_turno(
        &w.db,
        uuid,
        2,
        "applicant",
        "answer",
        "Segundo: mi respuesta",
    )
    .await;

    let (e, g) = ruta(&w, "GET", &id, "").await;
    assert_eq!(e, StatusCode::OK, "{g}");
    assert_eq!(g["id"], id);
    assert_eq!(g["status"], "in_progress");
    assert_eq!(g["recruiter"]["display_name"], "Ana");
    assert_eq!(g["recruiter"]["role_title"], "RRHH");
    assert_eq!(g["level"]["code"], "junior");
    let t = g["turns"].as_array().unwrap();
    assert_eq!(t.len(), 3);
    assert_eq!(
        t.iter()
            .map(|x| x["seq"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        t[0]["text"],
        primera_pregunta(RecruiterCode::Ana, Level::Junior)
    );
    assert_eq!(
        (t[1]["speaker"].as_str(), t[1]["kind"].as_str()),
        (Some("applicant"), Some("answer"))
    );
    assert_eq!(t[1]["text"], "Segundo: mi respuesta");
    assert_eq!(
        (t[2]["speaker"].as_str(), t[2]["kind"].as_str()),
        (Some("recruiter"), Some("follow_up"))
    );
    assert_eq!(t[2]["text"], "Tercero, con ñ y 🚀");
    w.db.cleanup().await;
}

// ================================================================== S9, S10, S11, S12

/// S9: pausar una en curso → 204 y `paused`; pausar una pausada o terminada → 409.
#[tokio::test]
async fn s9_pausar() {
    let w = mundo().await;
    let (_, c) = crear(&w, "ana", "junior").await;
    let id = c["id"].as_str().unwrap().to_string();
    let uuid: Uuid = id.parse().unwrap();

    let (e, cuerpo) = ruta(&w, "POST", &id, "/pause").await;
    assert_eq!((e, cuerpo), (StatusCode::NO_CONTENT, Value::Null));
    assert_eq!(estado_de(&w.db, uuid).await, "paused");
    assert_eq!(
        w.db.count(&format!(
            "SELECT count(*) FROM interview_sessions WHERE id = '{uuid}' AND paused_at IS NOT NULL"
        ))
        .await,
        1
    );

    let (e, cuerpo) = ruta(&w, "POST", &id, "/pause").await;
    assert_eq!(e, StatusCode::CONFLICT, "{cuerpo}");
    assert_eq!(error_de(&cuerpo), "estado_invalido");
    assert_eq!(estado_de(&w.db, uuid).await, "paused");

    for estado in ["completed", "abandoned"] {
        let s = sembrar(
            &w.db,
            w.applicant,
            "ana",
            "junior",
            estado,
            "now() - interval '1 day'",
        )
        .await;
        let (e, cuerpo) = ruta(&w, "POST", s, "/pause").await;
        assert_eq!(e, StatusCode::CONFLICT, "{estado}: {cuerpo}");
        assert_eq!(error_de(&cuerpo), "estado_invalido");
        assert_eq!(estado_de(&w.db, s).await, estado, "no se toca");
    }
    w.db.cleanup().await;
}

/// S10: retomar → 200 `{turn}` con el último turno del recruiter y la sesión vuelve a `in_progress`.
#[tokio::test]
async fn s10_retomar_devuelve_el_ultimo_turno_del_recruiter() {
    let w = mundo().await;
    let (_, c) = crear(&w, "ana", "junior").await;
    let id = c["id"].as_str().unwrap().to_string();
    let uuid: Uuid = id.parse().unwrap();
    // La conversación avanzó: respuesta de la persona y una repregunta (último turno del recruiter).
    sembrar_turno(&w.db, uuid, 2, "applicant", "answer", "respuesta corta").await;
    sembrar_turno(
        &w.db,
        uuid,
        3,
        "recruiter",
        "follow_up",
        "¿Puedes darme un ejemplo?",
    )
    .await;
    // La persona contestó otra vez y el último turno de TODOS es de la persona.
    sembrar_turno(&w.db, uuid, 4, "applicant", "answer", "otra respuesta").await;

    ruta(&w, "POST", &id, "/pause").await;
    let (e, r) = ruta(&w, "POST", &id, "/resume").await;
    assert_eq!(e, StatusCode::OK, "{r}");
    assert_eq!(r["turn"]["seq"], 3);
    assert_eq!(r["turn"]["speaker"], "recruiter");
    assert_eq!(r["turn"]["kind"], "follow_up");
    assert_eq!(r["turn"]["text"], "¿Puedes darme un ejemplo?");
    assert_eq!(estado_de(&w.db, uuid).await, "in_progress");
    assert_eq!(
        w.db.count(&format!(
            "SELECT count(*) FROM interview_sessions WHERE id = '{uuid}' AND paused_at IS NULL"
        ))
        .await,
        1,
        "al retomar se limpia paused_at"
    );
    w.db.cleanup().await;
}

/// S10: solo se retoma una pausada; en curso, terminada o abandonada → 409 sin tocar nada.
#[tokio::test]
async fn s10b_solo_se_retoma_una_pausada() {
    let w = mundo().await;
    let (_, c) = crear(&w, "ana", "junior").await;
    let id = c["id"].as_str().unwrap().to_string();
    let (e, r) = ruta(&w, "POST", &id, "/resume").await;
    assert_eq!(e, StatusCode::CONFLICT, "{r}");
    assert_eq!(error_de(&r), "estado_invalido");
    for estado in ["completed", "abandoned"] {
        let s = sembrar(&w.db, w.applicant, "ana", "junior", estado, "now()").await;
        let (e, r) = ruta(&w, "POST", s, "/resume").await;
        assert_eq!(e, StatusCode::CONFLICT, "{estado}: {r}");
        assert_eq!(estado_de(&w.db, s).await, estado);
    }
    w.db.cleanup().await;
}

/// S11: pausar y retomar varias veces no repite ni salta preguntas: no se crean turnos y la
/// pregunta que se devuelve es siempre la misma, la que estaba abierta.
#[tokio::test]
async fn s11_retomar_no_repite_ni_salta_preguntas() {
    let w = mundo().await;
    let (_, c) = crear(&w, "ana", "junior").await;
    let id = c["id"].as_str().unwrap().to_string();
    let uuid: Uuid = id.parse().unwrap();
    let primera = c["turns"][0].clone();

    for _ in 0..3 {
        assert_eq!(
            ruta(&w, "POST", &id, "/pause").await.0,
            StatusCode::NO_CONTENT
        );
        let (e, r) = ruta(&w, "POST", &id, "/resume").await;
        assert_eq!(e, StatusCode::OK);
        assert_eq!(
            r["turn"], primera,
            "la misma pregunta abierta, ni repetida ni saltada"
        );
    }
    assert_eq!(
        w.db.count(&format!(
            "SELECT count(*) FROM interview_turns WHERE interview_session_id = '{uuid}'"
        ))
        .await,
        1,
        "retomar no escribe turnos"
    );

    // Con la conversación avanzada, retomar devuelve la pregunta siguiente, no la primera.
    sembrar_turno(
        &w.db,
        uuid,
        2,
        "applicant",
        "answer",
        "Respondí con un ejemplo concreto de 3 proyectos",
    )
    .await;
    sembrar_turno(&w.db, uuid, 3, "recruiter", "question", "SEGUNDA PREGUNTA").await;
    ruta(&w, "POST", &id, "/pause").await;
    let (_, r) = ruta(&w, "POST", &id, "/resume").await;
    assert_eq!(r["turn"]["seq"], 3);
    assert_eq!(r["turn"]["text"], "SEGUNDA PREGUNTA");
    let (_, g) = ruta(&w, "GET", &id, "").await;
    assert_eq!(
        g["turns"].as_array().unwrap().len(),
        3,
        "la transcripción no cambió"
    );
    w.db.cleanup().await;
}

/// S12: abandonar (desde en curso o pausada) → 204 y libera el cupo; terminada o abandonada → 409.
#[tokio::test]
async fn s12_abandonar_libera_el_cupo() {
    let w = mundo().await;
    for pausar in [false, true] {
        let (_, c) = crear(&w, "ana", "junior").await;
        let id = c["id"].as_str().unwrap().to_string();
        let uuid: Uuid = id.parse().unwrap();
        if pausar {
            ruta(&w, "POST", &id, "/pause").await;
        }
        let (e, cuerpo) = ruta(&w, "POST", &id, "/abandon").await;
        assert_eq!((e, cuerpo), (StatusCode::NO_CONTENT, Value::Null));
        assert_eq!(estado_de(&w.db, uuid).await, "abandoned");
        // Abandonar otra vez → 409.
        let (e, cuerpo) = ruta(&w, "POST", &id, "/abandon").await;
        assert_eq!(e, StatusCode::CONFLICT, "{cuerpo}");
        assert_eq!(error_de(&cuerpo), "estado_invalido");
    }
    let (e, c) = crear(&w, "ana", "junior").await;
    assert_eq!(e, StatusCode::CREATED, "el cupo quedó libre: {c}");

    let terminada = sembrar(&w.db, w.applicant, "ana", "junior", "completed", "now()").await;
    let (e, c) = ruta(&w, "POST", terminada, "/abandon").await;
    assert_eq!(e, StatusCode::CONFLICT, "{c}");
    assert_eq!(estado_de(&w.db, terminada).await, "completed");
    w.db.cleanup().await;
}

// ================================================================== S13

async fn pausada_hace(db: &TestDb, applicant: Uuid, intervalo: &str) -> Uuid {
    let id = sembrar(
        db,
        applicant,
        "ana",
        "junior",
        "paused",
        "now() - interval '30 days'",
    )
    .await;
    db.exec(&format!("UPDATE interview_sessions SET paused_at = now() - interval '{intervalo}' WHERE id = '{id}'"))
        .await
        .unwrap();
    id
}

/// S13: una sesión pausada más de 7 días pasa a `abandoned`; el reloj es inyectable (sin dormir).
#[tokio::test]
async fn s13_la_limpieza_abandona_las_pausadas_de_mas_de_7_dias() {
    let w = mundo().await;
    let vieja = pausada_hace(&w.db, w.applicant, "8 days").await;
    let (_, otro) = common::nuevo_applicant(&w.db, "otro").await;
    let vieja_ajena = pausada_hace(&w.db, otro, "9 days").await;
    let (_, tercero) = common::nuevo_applicant(&w.db, "tercero").await;
    let reciente = pausada_hace(&w.db, tercero, "6 days 23 hours").await;
    let (_, cuarto) = common::nuevo_applicant(&w.db, "cuarto").await;
    let en_curso = sembrar(
        &w.db,
        cuarto,
        "ana",
        "junior",
        "in_progress",
        "now() - interval '40 days'",
    )
    .await;
    let completa = sembrar(
        &w.db,
        w.applicant,
        "ana",
        "junior",
        "completed",
        "now() - interval '40 days'",
    )
    .await;

    let n = interview_store::abandon_stale_paused(
        &w.db.conn,
        SystemTime::now(),
        interview_store::PAUSED_MAX_AGE,
    )
    .await
    .unwrap();
    assert_eq!(n, 2);
    assert_eq!(estado_de(&w.db, vieja).await, "abandoned");
    assert_eq!(estado_de(&w.db, vieja_ajena).await, "abandoned");
    assert_eq!(estado_de(&w.db, reciente).await, "paused");
    assert_eq!(
        estado_de(&w.db, en_curso).await,
        "in_progress",
        "solo las pausadas"
    );
    assert_eq!(estado_de(&w.db, completa).await, "completed");

    // Libera el cupo: ahora se puede crear.
    let (e, c) = crear(&w, "ana", "junior").await;
    assert_eq!(e, StatusCode::CREATED, "{c}");

    // Idempotente: otra pasada no encuentra nada.
    let n = interview_store::abandon_stale_paused(
        &w.db.conn,
        SystemTime::now(),
        interview_store::PAUSED_MAX_AGE,
    )
    .await
    .unwrap();
    assert_eq!(n, 0);
    w.db.cleanup().await;
}

/// S13: el umbral y el reloj se inyectan: con el reloj 8 días adelante, una pausada de hoy vence.
#[tokio::test]
async fn s13b_reloj_y_umbral_inyectables() {
    let w = mundo().await;
    let hoy = pausada_hace(&w.db, w.applicant, "1 minute").await;
    let ahora = SystemTime::now();

    let n =
        interview_store::abandon_stale_paused(&w.db.conn, ahora, interview_store::PAUSED_MAX_AGE)
            .await
            .unwrap();
    assert_eq!(n, 0);
    assert_eq!(estado_de(&w.db, hoy).await, "paused");

    // Umbral más corto: 30 segundos.
    let n = interview_store::abandon_stale_paused(&w.db.conn, ahora, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(estado_de(&w.db, hoy).await, "abandoned");

    // Reloj adelantado 8 días con el umbral de 7.
    let otra = pausada_hace(&w.db, w.applicant, "1 minute").await;
    let futuro = ahora + Duration::from_secs(8 * 24 * 3600);
    let n =
        interview_store::abandon_stale_paused(&w.db.conn, futuro, interview_store::PAUSED_MAX_AGE)
            .await
            .unwrap();
    assert_eq!(n, 1);
    assert_eq!(estado_de(&w.db, otra).await, "abandoned");
    assert_eq!(
        interview_store::PAUSED_MAX_AGE,
        Duration::from_secs(7 * 24 * 3600)
    );
    w.db.cleanup().await;
}

// ================================================================== S14

/// S14: borrar quita la sesión, sus turnos y su score; el progreso acumulado no cambia. Vale en
/// cualquier estado y libera el cupo.
#[tokio::test]
async fn s14_borrar_en_cascada_sin_tocar_el_progreso() {
    let w = mundo().await;
    let s = sembrar(
        &w.db,
        w.applicant,
        "ana",
        "junior",
        "completed",
        "now() - interval '1 day'",
    )
    .await;
    sembrar_turno(&w.db, s, 1, "recruiter", "question", "P").await;
    sembrar_turno(&w.db, s, 2, "applicant", "answer", "R").await;
    w.db.exec(&format!(
        "INSERT INTO interview_scores (id, interview_session_id, score, clarity, breakdown, rubric_version_id) \
         SELECT gen_random_uuid(), id, 80, 70, '[]'::jsonb, rubric_version_id FROM interview_sessions WHERE id = '{s}'"
    ))
    .await
    .unwrap();
    let junior = id_nivel(&w.db, "junior").await;
    w.db.exec(&format!(
        "INSERT INTO applicant_level_progress (id, applicant_id, interview_level_id, best_score, sessions_completed) \
         VALUES (gen_random_uuid(), '{}', '{junior}', 80, 1)",
        w.applicant
    ))
    .await
    .unwrap();
    let progreso = "SELECT count(*) FROM applicant_level_progress WHERE best_score = 80 AND sessions_completed = 1";
    assert_eq!(w.db.count(progreso).await, 1);

    let (e, c) = ruta(&w, "DELETE", s, "").await;
    assert_eq!((e, c), (StatusCode::NO_CONTENT, Value::Null));
    for tabla in ["interview_sessions", "interview_turns", "interview_scores"] {
        assert_eq!(
            w.db.count(&format!("SELECT count(*) FROM {tabla}")).await,
            0,
            "{tabla}"
        );
    }
    assert_eq!(w.db.count(progreso).await, 1, "el progreso no cambia");

    // Segunda vez: ya no existe.
    let (e, c) = ruta(&w, "DELETE", s, "").await;
    assert_eq!(e, StatusCode::NOT_FOUND, "{c}");

    // Una activa también se borra y libera el cupo.
    let (_, c) = crear(&w, "ana", "junior").await;
    let activa = c["id"].as_str().unwrap().to_string();
    assert_eq!(
        ruta(&w, "DELETE", &activa, "").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(crear(&w, "ana", "junior").await.0, StatusCode::CREATED);
    w.db.cleanup().await;
}

// ================================================================== X1

/// X1: la sesión de otra persona da 404, idéntico al de una inexistente, en TODAS las rutas, y
/// no se toca nada.
#[tokio::test]
async fn x1_la_sesion_ajena_es_404_identico_al_inexistente() {
    let w = mundo().await;
    let otro_token = registrar(&w.app, "intruso@correo-prueba.com").await;
    let ajena = sembrar(&w.db, w.applicant, "ana", "junior", "paused", "now()").await;
    sembrar_turno(&w.db, ajena, 1, "recruiter", "question", "SECRETO-AJENO").await;
    let inexistente = Uuid::now_v7();

    for (m, s) in [
        ("GET", ""),
        ("DELETE", ""),
        ("POST", "/pause"),
        ("POST", "/resume"),
        ("POST", "/abandon"),
    ] {
        let (e1, c1) = enviar(
            &w.app,
            m,
            &format!("/api/v1/interviews/{ajena}{s}"),
            Some(&otro_token),
            None,
        )
        .await;
        let (e2, c2) = enviar(
            &w.app,
            m,
            &format!("/api/v1/interviews/{inexistente}{s}"),
            Some(&otro_token),
            None,
        )
        .await;
        assert_eq!(e1, StatusCode::NOT_FOUND, "{m} {s}: {c1}");
        assert_eq!(
            (e1, &c1),
            (e2, &c2),
            "{m} {s}: ajena e inexistente deben ser idénticas"
        );
        assert_eq!(error_de(&c1), "no_encontrado");
        assert!(!c1.to_string().contains("SECRETO-AJENO"));
    }
    assert_eq!(estado_de(&w.db, ajena).await, "paused", "nada cambió");
    assert_eq!(w.db.count("SELECT count(*) FROM interview_turns").await, 1);

    // El listado del intruso no la trae.
    let (_, c) = enviar(&w.app, "GET", "/api/v1/interviews", Some(&otro_token), None).await;
    assert_eq!(c["items"].as_array().unwrap().len(), 0);
    // Y el dueño sí la lee.
    assert_eq!(ruta(&w, "GET", ajena, "").await.0, StatusCode::OK);
    w.db.cleanup().await;
}

// ================================================================== X2, X6

fn todas_las_rutas(id: Uuid) -> Vec<(&'static str, String)> {
    vec![
        ("POST", "/api/v1/interviews".to_string()),
        ("GET", "/api/v1/interviews".to_string()),
        ("GET", format!("/api/v1/interviews/{id}")),
        ("POST", format!("/api/v1/interviews/{id}/pause")),
        ("POST", format!("/api/v1/interviews/{id}/resume")),
        ("POST", format!("/api/v1/interviews/{id}/abandon")),
        ("DELETE", format!("/api/v1/interviews/{id}")),
    ]
}

/// X2: toda ruta de sesiones sin token (o con uno inválido) → 401.
#[tokio::test]
async fn x2_sin_token_todo_es_401() {
    let w = mundo().await;
    for (m, uri) in todas_las_rutas(Uuid::now_v7()) {
        assert_eq!(
            enviar(&w.app, m, &uri, None, None).await.0,
            StatusCode::UNAUTHORIZED,
            "{m} {uri}"
        );
        assert_eq!(
            enviar(&w.app, m, &uri, Some("basura"), None).await.0,
            StatusCode::UNAUTHORIZED,
            "{m} {uri}"
        );
    }
    w.db.cleanup().await;
}

/// X6: una cuenta suspendida recibe 403 `cuenta_no_disponible` en todas, y no crea nada.
#[tokio::test]
async fn x6_una_cuenta_suspendida_recibe_403() {
    let w = mundo().await;
    let (_, c) = crear(&w, "ana", "junior").await;
    let id: Uuid = c["id"].as_str().unwrap().parse().unwrap();
    let user = jwt().validate(&w.token).unwrap();
    w.db.exec(&format!(
        "UPDATE users SET status = 'suspended' WHERE id = '{user}'"
    ))
    .await
    .unwrap();

    for (m, uri) in todas_las_rutas(id) {
        let cuerpo = (m == "POST" && uri.ends_with("/interviews"))
            .then(|| json!({"recruiter_id": Uuid::now_v7(), "level_id": Uuid::now_v7()}));
        let (e, c) = enviar(&w.app, m, &uri, Some(&w.token), cuerpo).await;
        assert_eq!(e, StatusCode::FORBIDDEN, "{m} {uri}: {c}");
        assert_eq!(error_de(&c), "cuenta_no_disponible");
    }
    assert_eq!(estado_de(&w.db, id).await, "in_progress");
    w.db.cleanup().await;
}

// ================================================================== X3

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

/// Instala, en este hilo, un suscriptor con el filtro de producción que escribe en un búfer.
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

const MARCA: &str = "MARCA-9f3c-NO-DEBE-SALIR";

/// X3 (sanidad): la captura funciona. Sin esto, «no aparece en el log» no probaría nada.
#[tokio::test]
async fn x3a_la_captura_de_logs_funciona() {
    let (buffer, _guarda) = capturar_logs();
    tracing::error!(
        donde = "prueba_de_captura",
        detalle = MARCA,
        "evento de prueba"
    );
    let log = log_de(&buffer);
    assert!(log.contains(MARCA), "la captura no ve los eventos: {log:?}");
}

/// X3: ni el texto de un turno ni lo que la persona manda aparece en un log ni en el cuerpo de un
/// error, ni con una petición correcta ni con una que falla por dentro.
#[tokio::test]
async fn x3_ni_el_texto_ni_los_datos_de_la_persona_salen_en_logs_ni_errores() {
    let w = mundo().await;
    let (buffer, _guarda) = capturar_logs();

    // Un turno con el texto sensible, leído con una petición correcta.
    let (_, c) = crear(&w, "ana", "junior").await;
    let id = c["id"].as_str().unwrap().to_string();
    let uuid: Uuid = id.parse().unwrap();
    sembrar_turno(&w.db, uuid, 2, "applicant", "answer", MARCA).await;
    let (e, g) = ruta(&w, "GET", &id, "").await;
    assert_eq!(e, StatusCode::OK);
    assert!(g.to_string().contains(MARCA), "el dueño sí ve su texto");
    ruta(&w, "POST", &id, "/pause").await;
    ruta(&w, "POST", &id, "/resume").await;
    enviar(&w.app, "GET", "/api/v1/interviews", Some(&w.token), None).await;

    // Datos de la persona en el cuerpo y en la URL de peticiones que fallan.
    let (e, c1) = enviar(
        &w.app,
        "POST",
        "/api/v1/interviews",
        Some(&w.token),
        Some(json!({"recruiter_id": MARCA, "level_id": MARCA, "nota": MARCA})),
    )
    .await;
    assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY);
    let (_, c2) = enviar_crudo(
        &w.app,
        "/api/v1/interviews",
        &w.token,
        &format!("{{\"x\": \"{MARCA}"),
    )
    .await;
    for cuerpo in [&c1, &c2] {
        assert!(
            !cuerpo.to_string().contains(MARCA),
            "se coló en un error: {cuerpo}"
        );
    }

    // Un fallo interno: el turno guardado no se puede descifrar (otra clave).
    let mal = encrypt_turn_text(&common::llaves(), "otra cosa").unwrap();
    let mut mal = mal;
    mal[20] ^= 1;
    w.db.exec(&format!(
        "UPDATE interview_turns SET transcript = '\\x{}'::bytea WHERE interview_session_id = '{uuid}' AND seq = 2",
        common::hex(&mal)
    ))
    .await
    .unwrap();
    let (e, c5) = ruta(&w, "GET", &id, "").await;
    assert_eq!(e, StatusCode::INTERNAL_SERVER_ERROR, "{c5}");
    assert_eq!(error_de(&c5), "error_interno");
    assert!(!c5.to_string().contains(MARCA));

    let log = log_de(&buffer);
    assert!(
        log.contains("error interno"),
        "el fallo interno debía registrarse (por categoría): {log:?}"
    );
    assert!(!log.contains(MARCA), "el texto salió en el log: {log}");

    // Lo que viene en la URL no se refleja en los errores. (La URL misma la registra la capa de
    // trazas HTTP, igual que en el resto de la API; por eso va fuera de la captura.)
    drop(_guarda);
    let (_, c3) = enviar(
        &w.app,
        "GET",
        &format!("/api/v1/interviews?status={MARCA}&cursor={MARCA}"),
        Some(&w.token),
        None,
    )
    .await;
    let (_, c4) = enviar(
        &w.app,
        "GET",
        &format!("/api/v1/interviews/{MARCA}"),
        Some(&w.token),
        None,
    )
    .await;
    for cuerpo in [&c3, &c4] {
        assert!(
            !cuerpo.to_string().contains(MARCA),
            "se coló en un error: {cuerpo}"
        );
    }
    w.db.cleanup().await;
}

//! Casos de verdad R1–R7 (y X1, X2, X5, X6 para estas rutas) de docs/working/fase3-entrevista.md:
//! `GET /recruiters`, `GET /levels` y `GET /progress`. Contra PostgreSQL real; una base por prueba.
//! Las sesiones y los scores se insertan por SQL: no dependen de las rutas de sesiones.
//!
//! Se escribieron antes que la implementación, en rojo. Ver CONTRIBUTING.md.
//!
//! X1 (ids ajenos → 404) no aplica a estas tres rutas: no llevan ningún id en el path; su
//! equivalente (R7) es que los datos de otra persona nunca aparecen. X2 y X5 viven además en
//! `tests/route_table.rs`.

#[allow(dead_code)]
mod common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use common::TestDb;
use http_body_util::BodyExt;
use jobout_core_api::{app_with_state, crypto::PiiKeys, jwt::JwtKeys, state::AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

const CLAVE: &str = "una-clave-larga-123";
const RUTAS: [&str; 3] = ["/api/v1/recruiters", "/api/v1/levels", "/api/v1/progress"];

fn app(db: &TestDb) -> Router {
    let pii = Arc::new(
        PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
    );
    app_with_state(AppState {
        db: db.conn.clone(),
        pii,
        jwt: Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap()),
        blobs: jobout_core_api::blob::memory_store(),
    })
}

async fn enviar(
    app: &Router,
    metodo: &str,
    uri: &str,
    token: Option<&str>,
    cuerpo: Option<Value>,
) -> (StatusCode, Value, String) {
    let mut p = Request::builder()
        .method(metodo)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = token {
        p = p.header("authorization", format!("Bearer {t}"));
    }
    let cuerpo = cuerpo.map_or(Body::empty(), |j| Body::from(j.to_string()));
    let r = app.clone().oneshot(p.body(cuerpo).unwrap()).await.unwrap();
    let estado = r.status();
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let texto = String::from_utf8_lossy(&bytes).to_string();
    (
        estado,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        texto,
    )
}

async fn get(app: &Router, token: &str, ruta: &str) -> (StatusCode, Value, String) {
    enviar(app, "GET", ruta, Some(token), None).await
}

async fn registrar(app: &Router, email: &str) -> String {
    let (estado, cuerpo, _) = enviar(
        app,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(json!({"email": email, "password": CLAVE, "first_name": "Ana", "last_name": "Paz"})),
    )
    .await;
    assert_eq!(estado, StatusCode::CREATED, "{cuerpo}");
    cuerpo["access_token"].as_str().unwrap().to_string()
}

async fn con_cuenta() -> (TestDb, Router, String) {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let token = registrar(&app, "ana@correo-prueba.com").await;
    (db, app, token)
}

/// El applicant de la cuenta dueña del token.
async fn applicant_de(db: &TestDb, app: &Router, token: &str) -> String {
    let (_, me, _) = get(app, token, "/api/v1/me").await;
    let user = me["id"].as_str().unwrap();
    common::uuid_de(
        db,
        &format!("SELECT id FROM applicants WHERE user_id = '{user}'"),
    )
    .await
    .to_string()
}

async fn nivel_id(db: &TestDb, code: &str) -> String {
    common::uuid_de(
        db,
        &format!("SELECT id FROM interview_levels WHERE code = '{code}'"),
    )
    .await
    .to_string()
}

async fn recruiter_id(db: &TestDb, code: &str) -> String {
    common::uuid_de(
        db,
        &format!("SELECT id FROM ai_recruiters WHERE code = '{code}'"),
    )
    .await
    .to_string()
}

/// Inserta una sesión terminada con su score. `completada` es un instante SQL.
async fn sesion(
    db: &TestDb,
    applicant: &str,
    recruiter: &str,
    nivel: &str,
    estado: &str,
    completada: &str,
    score: Option<i32>,
) {
    let id = uuid::Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO interview_sessions \
         (id, applicant_id, ai_recruiter_id, interview_level_id, rubric_version_id, status, completed_at) \
         VALUES ('{id}', '{applicant}', '{recruiter}', '{nivel}', \
           (SELECT rv.id FROM rubric_versions rv JOIN ai_recruiters r ON r.rubric_id = rv.rubric_id \
            WHERE r.id = '{recruiter}' AND rv.is_current), '{estado}', {completada})"
    ))
    .await
    .unwrap();
    if let Some(s) = score {
        db.exec(&format!(
            "INSERT INTO interview_scores \
             (id, interview_session_id, score, clarity, breakdown, rubric_version_id) \
             VALUES ('{}', '{id}', {s}, 50, '[]'::jsonb, \
               (SELECT rubric_version_id FROM interview_sessions WHERE id = '{id}'))",
            uuid::Uuid::now_v7()
        ))
        .await
        .unwrap();
    }
}

async fn progreso_nivel(
    db: &TestDb,
    applicant: &str,
    nivel: &str,
    best: Option<i32>,
    completadas: i32,
) {
    let best = best.map_or("NULL".to_string(), |b| b.to_string());
    db.exec(&format!(
        "INSERT INTO applicant_level_progress \
         (id, applicant_id, interview_level_id, best_score, sessions_completed) \
         VALUES ('{}', '{applicant}', '{nivel}', {best}, {completadas})",
        uuid::Uuid::now_v7()
    ))
    .await
    .unwrap();
}

fn nivel<'a>(cuerpo: &'a Value, code: &str) -> &'a Value {
    cuerpo["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["code"] == code)
        .unwrap_or_else(|| panic!("falta el nivel {code}: {cuerpo}"))
}

// ------------------------------------------------------------ R1 / X2

/// R1 y X2: sin token (o con basura), las tres rutas dan 401 y no devuelven datos.
#[tokio::test]
async fn r1_sin_token_es_401() {
    let (db, app, _) = con_cuenta().await;
    for ruta in RUTAS {
        for token in [None, Some("esto.no.es-un-jwt")] {
            let (estado, cuerpo, texto) = enviar(&app, "GET", ruta, token, None).await;
            assert_eq!(estado, StatusCode::UNAUTHORIZED, "{ruta} {token:?}");
            assert!(cuerpo.get("items").is_none(), "{ruta}: {texto}");
        }
    }
    db.cleanup().await;
}

// ------------------------------------------------------------ R2

/// R2: los cinco recruiters con sus campos públicos, sin `system_prompt` ni la rúbrica.
#[tokio::test]
async fn r2_recruiters_con_campos_publicos() {
    let (db, app, token) = con_cuenta().await;
    // Un prompt con contenido: si se filtrara, aparecería en la respuesta.
    db.exec("UPDATE ai_recruiters SET system_prompt = 'PROMPT-INTERNO-SECRETO-77'")
        .await
        .unwrap();

    let (estado, cuerpo, texto) = get(&app, &token, "/api/v1/recruiters").await;
    assert_eq!(estado, StatusCode::OK, "{texto}");
    let items = cuerpo["items"].as_array().unwrap();
    let codes: Vec<&str> = items.iter().map(|i| i["code"].as_str().unwrap()).collect();
    assert_eq!(codes, ["ana", "martin", "laura", "carlos", "sofia"]);

    for i in items {
        let mut claves: Vec<&str> = i.as_object().unwrap().keys().map(String::as_str).collect();
        claves.sort_unstable();
        assert_eq!(
            claves,
            [
                "code",
                "display_name",
                "id",
                "language",
                "personality",
                "role_title"
            ],
            "{i}"
        );
        for campo in ["id", "display_name", "role_title", "personality"] {
            assert!(!i[campo].as_str().unwrap().is_empty(), "{campo} vacío: {i}");
        }
        let esperado = if i["code"] == "sofia" { "en" } else { "es" };
        assert_eq!(i["language"], esperado, "{i}");
    }
    assert_eq!(
        items[0]["id"].as_str().unwrap(),
        recruiter_id(&db, "ana").await
    );
    assert!(!texto.contains("PROMPT-INTERNO-SECRETO-77"), "{texto}");
    for prohibido in ["system_prompt", "criteria", "rubric", "voice_id"] {
        assert!(!texto.contains(prohibido), "filtra {prohibido}: {texto}");
    }
    db.cleanup().await;
}

/// Un recruiter inactivo no se ofrece.
#[tokio::test]
async fn r2b_un_recruiter_inactivo_no_se_ofrece() {
    let (db, app, token) = con_cuenta().await;
    db.exec("UPDATE ai_recruiters SET active = false WHERE code = 'carlos'")
        .await
        .unwrap();
    let (_, cuerpo, _) = get(&app, &token, "/api/v1/recruiters").await;
    assert_eq!(cuerpo["items"].as_array().unwrap().len(), 4);
    assert!(cuerpo["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["code"] != "carlos"));
    db.cleanup().await;
}

// ------------------------------------------------------------ R3 / R4

/// R4: una cuenta nueva ve los cinco niveles por posición y solo Junior desbloqueado.
#[tokio::test]
async fn r4_cuenta_nueva_solo_junior_desbloqueado() {
    let (db, app, token) = con_cuenta().await;
    let (estado, cuerpo, texto) = get(&app, &token, "/api/v1/levels").await;
    assert_eq!(estado, StatusCode::OK, "{texto}");
    let items = cuerpo["items"].as_array().unwrap();
    let codes: Vec<&str> = items.iter().map(|i| i["code"].as_str().unwrap()).collect();
    assert_eq!(codes, ["junior", "startup", "amazon", "google", "mckinsey"]);
    let posiciones: Vec<i64> = items
        .iter()
        .map(|i| i["position"].as_i64().unwrap())
        .collect();
    assert!(posiciones.windows(2).all(|w| w[0] < w[1]), "{posiciones:?}");

    for (i, n) in items.iter().enumerate() {
        let mut claves: Vec<&str> = n.as_object().unwrap().keys().map(String::as_str).collect();
        claves.sort_unstable();
        assert_eq!(
            claves,
            [
                "available",
                "best_score",
                "code",
                "id",
                "min_score_to_unlock",
                "name",
                "position",
                "sessions_completed",
                "unlocked"
            ],
            "{n}"
        );
        assert_eq!(n["unlocked"], i == 0, "{}", n["code"]);
        assert!(n["best_score"].is_null(), "{n}");
        assert_eq!(n["sessions_completed"], 0, "{n}");
    }
    assert_eq!(
        items[0]["id"].as_str().unwrap(),
        nivel_id(&db, "junior").await
    );
    assert_eq!(items[0]["name"], "Junior");
    assert!(items[0]["min_score_to_unlock"].is_null());
    assert!(items[1]["min_score_to_unlock"].as_i64().unwrap() >= 1);
    assert_eq!(items[0]["available"], true);
    assert_eq!(items[1]["available"], true);
    assert_eq!(items[2]["available"], false);
    db.cleanup().await;
}

/// R3: para **mi** cuenta, qué niveles están desbloqueados, el mejor score y las sesiones
/// completadas.
#[tokio::test]
async fn r3_niveles_con_mi_progreso() {
    let (db, app, token) = con_cuenta().await;
    let ap = applicant_de(&db, &app, &token).await;
    progreso_nivel(&db, &ap, &nivel_id(&db, "junior").await, Some(85), 3).await;
    progreso_nivel(&db, &ap, &nivel_id(&db, "startup").await, None, 0).await;

    let (_, cuerpo, _) = get(&app, &token, "/api/v1/levels").await;
    let junior = nivel(&cuerpo, "junior");
    assert_eq!(junior["unlocked"], true);
    assert_eq!(junior["best_score"], 85);
    assert_eq!(junior["sessions_completed"], 3);
    // Una fila de progreso sin score (desbloqueado, sin terminar nada) → best_score null.
    let startup = nivel(&cuerpo, "startup");
    assert_eq!(startup["unlocked"], true);
    assert!(startup["best_score"].is_null());
    assert_eq!(startup["sessions_completed"], 0);
    // Sin fila de progreso, un nivel con previo sigue bloqueado.
    assert_eq!(nivel(&cuerpo, "amazon")["unlocked"], false);
    db.cleanup().await;
}

/// R3: el progreso de una cuenta no cambia lo que ve otra.
#[tokio::test]
async fn r3b_los_niveles_son_por_cuenta() {
    let (db, app, token) = con_cuenta().await;
    let otro = registrar(&app, "luis@correo-prueba.com").await;
    let ap_otro = applicant_de(&db, &app, &otro).await;
    progreso_nivel(&db, &ap_otro, &nivel_id(&db, "startup").await, Some(90), 5).await;

    let (_, mio, _) = get(&app, &token, "/api/v1/levels").await;
    let startup = nivel(&mio, "startup");
    assert_eq!(startup["unlocked"], false);
    assert!(startup["best_score"].is_null());
    assert_eq!(startup["sessions_completed"], 0);
    let (_, suyo, _) = get(&app, &otro, "/api/v1/levels").await;
    assert_eq!(nivel(&suyo, "startup")["unlocked"], true);
    assert_eq!(nivel(&suyo, "startup")["best_score"], 90);
    db.cleanup().await;
}

// ------------------------------------------------------------ R5

/// R5: un nivel no `available` aparece bloqueado aunque el progreso (o el score) alcance.
#[tokio::test]
async fn r5_un_nivel_no_disponible_aparece_bloqueado() {
    let (db, app, token) = con_cuenta().await;
    let ap = applicant_de(&db, &app, &token).await;
    // Amazon, Google y McKinsey no están disponibles, pero la persona tiene fila de progreso.
    for code in ["amazon", "google", "mckinsey"] {
        progreso_nivel(&db, &ap, &nivel_id(&db, code).await, Some(99), 4).await;
    }
    let (_, cuerpo, _) = get(&app, &token, "/api/v1/levels").await;
    for code in ["amazon", "google", "mckinsey"] {
        let n = nivel(&cuerpo, code);
        assert_eq!(n["available"], false, "{code}");
        assert_eq!(n["unlocked"], false, "{code} no se puede empezar: {n}");
    }
    // Y si Junior deja de estar disponible, también queda bloqueado.
    db.exec("UPDATE interview_levels SET available = false WHERE code = 'junior'")
        .await
        .unwrap();
    let (_, cuerpo, _) = get(&app, &token, "/api/v1/levels").await;
    assert_eq!(nivel(&cuerpo, "junior")["unlocked"], false);
    db.cleanup().await;
}

// ------------------------------------------------------------ R6 / R7

/// R6: la serie de mis sesiones completadas, en orden cronológico ascendente, con sus campos.
/// Las no completadas no aparecen.
#[tokio::test]
async fn r6_progreso_cronologico() {
    let (db, app, token) = con_cuenta().await;
    let ap = applicant_de(&db, &app, &token).await;
    let (ana, martin) = (
        recruiter_id(&db, "ana").await,
        recruiter_id(&db, "martin").await,
    );
    let (junior, startup) = (
        nivel_id(&db, "junior").await,
        nivel_id(&db, "startup").await,
    );

    // Insertadas desordenadas a propósito.
    sesion(
        &db,
        &ap,
        &martin,
        &startup,
        "completed",
        "now() - interval '1 day'",
        Some(70),
    )
    .await;
    sesion(
        &db,
        &ap,
        &ana,
        &junior,
        "completed",
        "now() - interval '3 days'",
        Some(55),
    )
    .await;
    sesion(
        &db,
        &ap,
        &ana,
        &junior,
        "completed",
        "now() - interval '2 days'",
        Some(62),
    )
    .await;
    sesion(&db, &ap, &ana, &junior, "abandoned", "NULL", None).await;
    sesion(&db, &ap, &ana, &junior, "paused", "NULL", None).await;

    let (estado, cuerpo, texto) = get(&app, &token, "/api/v1/progress").await;
    assert_eq!(estado, StatusCode::OK, "{texto}");
    let items = cuerpo["items"].as_array().unwrap();
    let scores: Vec<i64> = items.iter().map(|i| i["score"].as_i64().unwrap()).collect();
    assert_eq!(scores, [55, 62, 70], "{texto}");
    let mut claves: Vec<&str> = items[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    claves.sort_unstable();
    assert_eq!(
        claves,
        ["completed_at", "level", "recruiter", "score", "session_id"]
    );
    assert_eq!(items[0]["level"], "Junior");
    assert_eq!(items[0]["recruiter"], "Ana");
    assert_eq!(items[2]["level"], "Startup");
    assert_eq!(items[2]["recruiter"], "Martín");
    assert!(items[0]["completed_at"].as_str().unwrap().contains('T'));
    assert!(items[0]["session_id"].as_str().unwrap().len() == 36);
    db.cleanup().await;
}

/// R6: una cuenta nueva tiene la serie vacía (no 404).
#[tokio::test]
async fn r6b_cuenta_nueva_sin_progreso() {
    let (db, app, token) = con_cuenta().await;
    let (estado, cuerpo, _) = get(&app, &token, "/api/v1/progress").await;
    assert_eq!(estado, StatusCode::OK);
    assert_eq!(cuerpo, json!({"items": []}));
    db.cleanup().await;
}

/// R6: devuelve las 200 sesiones completadas **más recientes**, en orden cronológico.
#[tokio::test]
async fn r6c_tope_de_200_las_mas_recientes() {
    let (db, app, token) = con_cuenta().await;
    let ap = applicant_de(&db, &app, &token).await;
    let (ana, junior) = (
        recruiter_id(&db, "ana").await,
        nivel_id(&db, "junior").await,
    );
    for i in 0..205 {
        // La más antigua (i = 0) tiene score 1; la más reciente, 100 + algo: usamos i % 100.
        sesion(
            &db,
            &ap,
            &ana,
            &junior,
            "completed",
            &format!("now() - interval '{} minutes'", 300 - i),
            Some(i % 100),
        )
        .await;
    }
    let (_, cuerpo, _) = get(&app, &token, "/api/v1/progress").await;
    let items = cuerpo["items"].as_array().unwrap();
    assert_eq!(items.len(), 200);
    // Se descartan las 5 más antiguas (i = 0..4): la primera es i = 5.
    assert_eq!(items[0]["score"], 5);
    assert_eq!(items[199]["score"], 204 % 100);
    let fechas: Vec<&str> = items
        .iter()
        .map(|i| i["completed_at"].as_str().unwrap())
        .collect();
    assert!(fechas.windows(2).all(|w| w[0] <= w[1]));
    db.cleanup().await;
}

/// R7: el progreso de otra persona nunca aparece en el mío.
#[tokio::test]
async fn r7_el_progreso_ajeno_no_aparece() {
    let (db, app, token) = con_cuenta().await;
    let otro = registrar(&app, "luis@correo-prueba.com").await;
    let (ap, ap_otro) = (
        applicant_de(&db, &app, &token).await,
        applicant_de(&db, &app, &otro).await,
    );
    let (ana, junior) = (
        recruiter_id(&db, "ana").await,
        nivel_id(&db, "junior").await,
    );
    sesion(&db, &ap, &ana, &junior, "completed", "now()", Some(41)).await;
    sesion(&db, &ap_otro, &ana, &junior, "completed", "now()", Some(97)).await;
    sesion(
        &db,
        &ap_otro,
        &ana,
        &junior,
        "completed",
        "now() - interval '1 hour'",
        Some(96),
    )
    .await;

    let (_, mio, texto) = get(&app, &token, "/api/v1/progress").await;
    let scores: Vec<i64> = mio["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["score"].as_i64().unwrap())
        .collect();
    assert_eq!(scores, [41], "{texto}");
    let (_, suyo, _) = get(&app, &otro, "/api/v1/progress").await;
    assert_eq!(suyo["items"].as_array().unwrap().len(), 2);
    db.cleanup().await;
}

// ------------------------------------------------------------ X6

/// X6: una cuenta suspendida con token vigente recibe 403 en las tres rutas, sin datos.
#[tokio::test]
async fn x6_suspendido_recibe_403() {
    let (db, app, token) = con_cuenta().await;
    db.exec("UPDATE users SET status = 'suspended'")
        .await
        .unwrap();
    for ruta in RUTAS {
        let (estado, cuerpo, texto) = get(&app, &token, ruta).await;
        assert_eq!(estado, StatusCode::FORBIDDEN, "{ruta}: {texto}");
        assert_eq!(cuerpo["error"], "cuenta_no_disponible");
        assert!(cuerpo.get("items").is_none());
    }
    db.cleanup().await;
}

/// Solo `GET`: el resto de métodos no existe en estas rutas.
#[tokio::test]
async fn solo_get() {
    let (db, app, token) = con_cuenta().await;
    for ruta in RUTAS {
        for m in ["POST", "PUT", "PATCH", "DELETE"] {
            let (estado, _, _) = enviar(&app, m, ruta, Some(&token), None).await;
            assert_eq!(estado, StatusCode::METHOD_NOT_ALLOWED, "{m} {ruta}");
        }
    }
    db.cleanup().await;
}

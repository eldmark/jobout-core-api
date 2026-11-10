//! Casos B1–B3 del paso 10 de docs/working/fase3-entrevista.md: borrar la cuenta también borra
//! las entrevistas (sesiones, turnos, scores y progreso) y deja el costo agregado sin el id de la
//! persona. Contra PostgreSQL real; una base por prueba.
//!
//! Se escribieron antes que la implementación, en rojo.

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
use jobout_core_api::{
    app_with_state, crypto::PiiKeys, interview::store_crypto::encrypt_turn_text, jwt::JwtKeys,
    state::AppState,
};
use serde_json::{json, Value};
use tower::ServiceExt;
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
    cuerpo: Value,
) -> (StatusCode, Value) {
    let mut peticion = Request::builder()
        .method(metodo)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = token {
        peticion = peticion.header("authorization", format!("Bearer {t}"));
    }
    let r = app
        .clone()
        .oneshot(peticion.body(Body::from(cuerpo.to_string())).unwrap())
        .await
        .unwrap();
    let estado = r.status();
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    (
        estado,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

struct Persona {
    email: String,
    token: String,
    user: Uuid,
    applicant: Uuid,
}

async fn registrar(db: &TestDb, app: &Router, email: &str) -> Persona {
    let (e, c) = enviar(
        app,
        "POST",
        "/api/v1/auth/register",
        None,
        json!({"email": email, "password": CLAVE, "first_name": "Ana", "last_name": "Prueba"}),
    )
    .await;
    assert_eq!(e, StatusCode::CREATED, "{c}");
    let token = c["access_token"].as_str().unwrap().to_string();
    let user: Uuid = jwt().validate(&token).unwrap().parse().unwrap();
    let applicant = common::uuid_de(
        db,
        &format!("SELECT id FROM applicants WHERE user_id = '{user}'"),
    )
    .await;
    Persona {
        email: email.to_string(),
        token,
        user,
        applicant,
    }
}

/// Siembra, para una persona: `n` sesiones (la última completada con score) con 2 turnos cada una,
/// filas de progreso en dos niveles y una fila de uso por sesión (costo `costo` cada una).
async fn sembrar(db: &TestDb, p: &Persona, n: usize, costo: i64) {
    for i in 0..n {
        let sesion = Uuid::now_v7();
        let estado = if i + 1 == n { "completed" } else { "abandoned" };
        db.exec(&format!(
            "INSERT INTO interview_sessions (id, applicant_id, ai_recruiter_id, interview_level_id, \
                rubric_version_id, status, started_at, completed_at) \
             SELECT '{sesion}', '{}', r.id, l.id, \
                (SELECT v.id FROM rubric_versions v WHERE v.rubric_id = r.rubric_id AND v.is_current), \
                '{estado}'::interview_status, now(), CASE WHEN '{estado}' = 'completed' THEN now() END \
             FROM ai_recruiters r, interview_levels l WHERE r.code = 'ana' AND l.code = 'junior'",
            p.applicant
        ))
        .await
        .unwrap();
        for (seq, speaker, kind) in [(1, "recruiter", "question"), (2, "applicant", "answer")] {
            let cifrado = encrypt_turn_text(&common::llaves(), "texto de la sesión").unwrap();
            db.exec(&format!(
                "INSERT INTO interview_turns (id, interview_session_id, seq, speaker, kind, transcript) \
                 VALUES ('{}', '{sesion}', {seq}, '{speaker}'::interview_speaker, '{kind}', '\\x{}'::bytea)",
                Uuid::now_v7(),
                common::hex(&cifrado)
            ))
            .await
            .unwrap();
        }
        if estado == "completed" {
            db.exec(&format!(
                "INSERT INTO interview_scores (id, interview_session_id, score, clarity, breakdown, rubric_version_id) \
                 SELECT '{}', '{sesion}', 80, 70, '[]'::jsonb, rubric_version_id \
                 FROM interview_sessions WHERE id = '{sesion}'",
                Uuid::now_v7()
            ))
            .await
            .unwrap();
        }
        db.exec(&format!(
            "INSERT INTO usage_ledger (id, user_id, interview_session_id, provider, operation, units, cost_micros) \
             VALUES ('{}', '{}', '{sesion}', 'deterministic', 'interview_session', 1, {costo})",
            Uuid::now_v7(),
            p.user
        ))
        .await
        .unwrap();
    }
    for nivel in ["junior", "startup"] {
        db.exec(&format!(
            "INSERT INTO applicant_level_progress (id, applicant_id, interview_level_id, best_score, sessions_completed) \
             SELECT '{}', '{}', id, 80, 1 FROM interview_levels WHERE code = '{nivel}'",
            Uuid::now_v7(),
            p.applicant
        ))
        .await
        .unwrap();
    }
}

async fn borrar(app: &Router, p: &Persona) {
    let (e, c) = enviar(
        app,
        "DELETE",
        "/api/v1/me",
        Some(&p.token),
        json!({"password": CLAVE}),
    )
    .await;
    assert_eq!(e, StatusCode::NO_CONTENT, "{c}");
}

async fn de(db: &TestDb, tabla_sql: &str, applicant: Uuid) -> i64 {
    db.count(&tabla_sql.replace("{a}", &applicant.to_string()))
        .await
}

const SESIONES: &str = "SELECT count(*) FROM interview_sessions WHERE applicant_id = '{a}'";
const TURNOS: &str = "SELECT count(*) FROM interview_turns t JOIN interview_sessions s \
                      ON s.id = t.interview_session_id WHERE s.applicant_id = '{a}'";
const SCORES: &str = "SELECT count(*) FROM interview_scores c JOIN interview_sessions s \
                      ON s.id = c.interview_session_id WHERE s.applicant_id = '{a}'";
const PROGRESO: &str = "SELECT count(*) FROM applicant_level_progress WHERE applicant_id = '{a}'";

/// B1: `DELETE /me` borra todas mis sesiones, turnos, scores y progreso; los de otros no se tocan.
#[tokio::test]
async fn b1_borrar_la_cuenta_borra_mis_entrevistas_y_no_las_ajenas() {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let yo = registrar(&db, &app, "ana.borra@correo-prueba.com").await;
    let otra = registrar(&db, &app, "luis.queda@correo-prueba.com").await;
    sembrar(&db, &yo, 3, 0).await;
    sembrar(&db, &otra, 2, 0).await;
    assert_eq!(
        (
            de(&db, SESIONES, yo.applicant).await,
            de(&db, TURNOS, yo.applicant).await,
            de(&db, SCORES, yo.applicant).await,
            de(&db, PROGRESO, yo.applicant).await
        ),
        (3, 6, 1, 2)
    );

    borrar(&app, &yo).await;

    for sql in [SESIONES, TURNOS, SCORES, PROGRESO] {
        assert_eq!(de(&db, sql, yo.applicant).await, 0, "{sql}");
    }
    // Lo de la otra persona, intacto: 2 sesiones, 4 turnos, 1 score, 2 niveles.
    assert_eq!(
        (
            de(&db, SESIONES, otra.applicant).await,
            de(&db, TURNOS, otra.applicant).await,
            de(&db, SCORES, otra.applicant).await,
            de(&db, PROGRESO, otra.applicant).await
        ),
        (2, 4, 1, 2)
    );
    assert_eq!(db.count("SELECT count(*) FROM interview_sessions").await, 2);
    assert_eq!(db.count("SELECT count(*) FROM interview_turns").await, 4);
    db.cleanup().await;
}

/// B2: `privacy_events` sigue con una sola constancia, sin datos personales ni ids de entrevista.
#[tokio::test]
async fn b2_queda_una_sola_constancia_sin_datos_personales() {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let yo = registrar(&db, &app, "ana.borra@correo-prueba.com").await;
    sembrar(&db, &yo, 2, 0).await;
    let sesion = common::uuid_de(&db, "SELECT id FROM interview_sessions LIMIT 1").await;
    borrar(&app, &yo).await;

    assert_eq!(db.count("SELECT count(*) FROM privacy_events").await, 1);
    let hash = pii().email_hash(&yo.email);
    let hex = common::hex(&hash);
    assert_eq!(
        db.count(&format!(
            "SELECT count(*) FROM privacy_events WHERE kind = 'account_deleted' \
             AND subject_hash = decode('{hex}', 'hex')"
        ))
        .await,
        1
    );
    for dato in [
        yo.user.to_string(),
        yo.applicant.to_string(),
        sesion.to_string(),
        "ana.borra".to_string(),
        "Prueba".to_string(),
    ] {
        assert_eq!(
            db.count(&format!(
                "SELECT count(*) FROM privacy_events p WHERE p::text ILIKE '%{dato}%'"
            ))
            .await,
            0,
            "la constancia lleva {dato}"
        );
    }
    db.cleanup().await;
}

/// B3: tras borrar la cuenta, `usage_ledger` conserva el costo agregado sin el id de la persona,
/// y el de otras personas conserva el suyo.
#[tokio::test]
async fn b3_el_ledger_conserva_el_costo_sin_el_id_de_la_persona() {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let yo = registrar(&db, &app, "ana.borra@correo-prueba.com").await;
    let otra = registrar(&db, &app, "luis.queda@correo-prueba.com").await;
    sembrar(&db, &yo, 3, 1_500).await;
    sembrar(&db, &otra, 2, 40_000).await;
    assert_eq!(
        db.count("SELECT sum(cost_micros)::bigint FROM usage_ledger")
            .await,
        3 * 1_500 + 2 * 40_000
    );

    borrar(&app, &yo).await;

    // Las filas siguen (aunque sus sesiones ya no), con el mismo costo total ...
    assert_eq!(db.count("SELECT count(*) FROM usage_ledger").await, 5);
    assert_eq!(
        db.count("SELECT sum(cost_micros)::bigint FROM usage_ledger")
            .await,
        3 * 1_500 + 2 * 40_000
    );
    // ... las mías sin nadie detrás: ni id de usuario ni de sesión ...
    assert_eq!(
        db.count(
            "SELECT count(*) FROM usage_ledger WHERE user_id IS NULL AND interview_session_id IS NULL \
             AND cost_micros = 1500"
        )
        .await,
        3
    );
    assert_eq!(
        db.count(&format!(
            "SELECT count(*) FROM usage_ledger l WHERE l::text ILIKE '%{}%' OR l::text ILIKE '%{}%'",
            yo.user, yo.applicant
        ))
        .await,
        0
    );
    // ... y las de la otra persona intactas.
    assert_eq!(
        db.count(&format!(
            "SELECT count(*) FROM usage_ledger WHERE user_id = '{}' AND interview_session_id IS NOT NULL",
            otra.user
        ))
        .await,
        2
    );
    db.cleanup().await;
}

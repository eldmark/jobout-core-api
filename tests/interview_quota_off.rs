//! Q7 por HTTP: con `ENFORCE_PLAN_LIMITS=false` la cuota no bloquea, pero el uso se registra.
//! Va en su propio binario de pruebas porque cambia una variable de entorno del proceso.

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
    let r = app
        .clone()
        .oneshot(peticion.body(cuerpo).unwrap())
        .await
        .unwrap();
    let estado = r.status();
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    (
        estado,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn q7_con_el_interruptor_apagado_no_bloquea_pero_registra() {
    std::env::set_var("ENFORCE_PLAN_LIMITS", "false");
    let db = TestDb::migrated().await;
    let app = app_with_state(AppState {
        db: db.conn.clone(),
        pii: Arc::new(
            PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
        ),
        jwt: Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap()),
        blobs: jobout_core_api::blob::memory_store(),
        engine: jobout_core_api::interview::default_engine(),
    });
    let (e, c) = enviar(
        &app,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(
            json!({"email": "ana.off@correo-prueba.com", "password": "una-clave-larga-123",
                    "first_name": "Ana", "last_name": "Prueba"}),
        ),
    )
    .await;
    assert_eq!(e, StatusCode::CREATED, "{c}");
    let token = c["access_token"].as_str().unwrap().to_string();
    let r = common::uuid_de(&db, "SELECT id FROM ai_recruiters WHERE code = 'ana'").await;
    let n = common::uuid_de(&db, "SELECT id FROM interview_levels WHERE code = 'junior'").await;

    for _ in 0..3 {
        let (e, c) = enviar(
            &app,
            "POST",
            "/api/v1/interviews",
            Some(&token),
            Some(json!({"recruiter_id": r, "level_id": n})),
        )
        .await;
        assert_eq!(e, StatusCode::CREATED, "{c}");
        let id = c["id"].as_str().unwrap();
        let (e, _) = enviar(
            &app,
            "POST",
            &format!("/api/v1/interviews/{id}/abandon"),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(e, StatusCode::NO_CONTENT);
    }
    // El plan era Gratis (tope 1) y aun así se registraron las tres.
    assert_eq!(
        db.count("SELECT count(*) FROM usage_ledger WHERE operation = 'interview_session'")
            .await,
        3
    );
    db.cleanup().await;
}

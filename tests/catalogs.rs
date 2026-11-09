//! Casos de verdad del endpoint de catálogos (K7–K9 de docs/working/fase2-perfil-cv.md).
//! Contra PostgreSQL real; el token sale del registro por la API real.

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

fn app(db: &TestDb) -> Router {
    app_with_state(AppState {
        db: db.conn.clone(),
        pii: Arc::new(
            PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
        ),
        jwt: Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap()),
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

async fn con_cuenta() -> (TestDb, Router, String) {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let (estado, cuerpo) = enviar(
        &app,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(
            json!({"email": "ana.lopez@correo-prueba.com", "password": "una-clave-larga-123",
                    "first_name": "Ana", "last_name": "Prueba"}),
        ),
    )
    .await;
    assert_eq!(estado, StatusCode::CREATED, "{cuerpo}");
    let token = cuerpo["access_token"].as_str().unwrap().to_string();
    (db, app, token)
}

/// K7: sin token (o con uno malo) → 401, en todos los catálogos.
#[tokio::test]
async fn k7_sin_token_es_401() {
    let (db, app, _) = con_cuenta().await;
    for nombre in [
        "skills",
        "languages",
        "academic-levels",
        "professions",
        "job-hierarchies",
    ] {
        let uri = format!("/api/v1/catalogs/{nombre}");
        let (e, c) = enviar(&app, "GET", &uri, None, None).await;
        assert_eq!(e, StatusCode::UNAUTHORIZED, "{nombre}");
        assert_eq!(c["error"], "no_autenticado");
        let (e, _) = enviar(&app, "GET", &uri, Some("esto.no.es-un-jwt"), None).await;
        assert_eq!(e, StatusCode::UNAUTHORIZED);
    }
    db.cleanup().await;
}

/// K8: con token devuelve solo las activas, por `sort_order` y luego `name`, con `id`, `code`
/// y `name` y nada más.
#[tokio::test]
async fn k8_devuelve_las_activas_ordenadas() {
    let (db, app, token) = con_cuenta().await;
    // Una inactiva, y tres filas al inicio del orden: dos empatadas en sort_order (desempata el
    // nombre, insertadas al revés) y una antes que todas.
    db.exec(
        "INSERT INTO skills (id, code, name, active, sort_order) VALUES \
         ('0192b3a0-0000-7000-8000-000000000001', 'zz-inactiva', 'AAA Inactiva', false, -10), \
         ('0192b3a0-0000-7000-8000-000000000002', 'zz-b', 'AAB Empate', true, -5), \
         ('0192b3a0-0000-7000-8000-000000000003', 'zz-a', 'AAA Empate', true, -5), \
         ('0192b3a0-0000-7000-8000-000000000004', 'zz-primera', 'ZZZ Primera', true, -6)",
    )
    .await
    .unwrap();

    let (estado, cuerpo) = enviar(&app, "GET", "/api/v1/catalogs/skills", Some(&token), None).await;
    assert_eq!(estado, StatusCode::OK, "{cuerpo}");
    let filas = cuerpo.as_array().expect("una lista");
    let codigos: Vec<&str> = filas.iter().map(|f| f["code"].as_str().unwrap()).collect();
    assert_eq!(&codigos[..3], ["zz-primera", "zz-a", "zz-b"]);
    assert!(
        !codigos.contains(&"zz-inactiva"),
        "no debe salir una inactiva"
    );
    assert_eq!(
        filas.len(),
        3 + db
            .count("SELECT count(*) FROM skills WHERE active AND code NOT LIKE 'zz-%'")
            .await as usize
    );

    // Forma exacta de cada elemento.
    let primera = filas[0].as_object().unwrap();
    let mut claves: Vec<&String> = primera.keys().collect();
    claves.sort();
    assert_eq!(claves, ["code", "id", "name"]);
    assert_eq!(primera["id"], "0192b3a0-0000-7000-8000-000000000004");
    assert_eq!(primera["name"], "ZZZ Primera");

    // Los demás respetan el orden (sort_order, name) del resto y traen contenido real.
    let rust = filas.iter().find(|f| f["code"] == "rust").expect("rust");
    assert_eq!(rust["name"], "Rust");

    for (nombre, esperado) in [
        ("languages", "es"),
        ("academic-levels", ""),
        ("professions", ""),
        ("job-hierarchies", ""),
    ] {
        let (e, c) = enviar(
            &app,
            "GET",
            &format!("/api/v1/catalogs/{nombre}"),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(e, StatusCode::OK, "{nombre}");
        let l = c.as_array().unwrap();
        assert!(!l.is_empty(), "{nombre} vacío");
        if !esperado.is_empty() {
            assert!(l.iter().any(|f| f["code"] == esperado));
        }
    }
    // Un catálogo vacío de activos devuelve una lista vacía, no un error.
    db.exec("UPDATE languages SET active = false")
        .await
        .unwrap();
    let (e, c) = enviar(
        &app,
        "GET",
        "/api/v1/catalogs/languages",
        Some(&token),
        None,
    )
    .await;
    assert_eq!((e, c), (StatusCode::OK, json!([])));
    db.cleanup().await;
}

/// K9: un catálogo desconocido → 404 `no_encontrado`; el nombre no se interpola en SQL.
#[tokio::test]
async fn k9_catalogo_desconocido_es_404() {
    let (db, app, token) = con_cuenta().await;
    for nombre in [
        "otra-cosa",
        "users",
        "Skills",
        "academic_levels",
        "skills%3Bselect",
        "pg_catalog",
    ] {
        let (e, c) = enviar(
            &app,
            "GET",
            &format!("/api/v1/catalogs/{nombre}"),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(e, StatusCode::NOT_FOUND, "{nombre}");
        assert_eq!(c["error"], "no_encontrado", "{nombre}");
        assert!(c["message"].as_str().unwrap().len() > 5);
    }
    db.cleanup().await;
}

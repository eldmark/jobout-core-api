//! Casos de verdad de la ruta de salud.
//!
//! Se escribieron antes que la implementación, en rojo. Ver CONTRIBUTING.md.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use jobout_core_api::app;
use tower::ServiceExt;

/// Caso 1: GET /health responde 200 con status "ok".
#[tokio::test]
async fn health_responde_ok() {
    let respuesta = app()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(respuesta.status(), StatusCode::OK);

    let cuerpo = respuesta.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&cuerpo).unwrap();

    assert_eq!(json["status"], "ok");
    assert_eq!(json["service"], "jobout-core-api");
}

/// Caso 2: /health no expone ningún dato de usuario.
///
/// Es una ruta pública. Todo lo que devuelva es visible sin autenticación,
/// así que la prueba fija qué campos puede traer.
#[tokio::test]
async fn health_no_expone_datos_de_usuario() {
    let respuesta = app()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let cuerpo = respuesta.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&cuerpo).unwrap();

    let permitidos = ["status", "service", "version"];
    for campo in json.as_object().unwrap().keys() {
        assert!(
            permitidos.contains(&campo.as_str()),
            "/health devolvió el campo inesperado '{campo}'"
        );
    }
}

/// Caso 3: una ruta que no existe responde 404, no 500.
#[tokio::test]
async fn ruta_inexistente_responde_404() {
    let respuesta = app()
        .oneshot(
            Request::builder()
                .uri("/no-existe")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(respuesta.status(), StatusCode::NOT_FOUND);
}

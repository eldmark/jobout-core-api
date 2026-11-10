//! Casos de verdad de renovación y cierre de sesión (R12–R15, más S3/S4/S6 aplicados a estas
//! rutas, de docs/working/fase1-identidad.md). Contra PostgreSQL real; una base por prueba.
//!
//! Se escribieron antes que la implementación, en rojo. Ver CONTRIBUTING.md.

mod common;

use std::sync::{Arc, Mutex};

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
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const EMAIL: &str = "ana.lopez@correo-prueba.com";
const CLAVE: &str = "una-clave-larga-123";
const REFRESH: &str = "/api/v1/auth/refresh";
const LOGOUT: &str = "/api/v1/auth/logout";

fn jwt() -> Arc<JwtKeys> {
    Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap())
}

fn app(db: &TestDb) -> Router {
    let pii = Arc::new(
        PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
    );
    app_with_state(AppState {
        db: db.conn.clone(),
        pii,
        jwt: jwt(),
        blobs: jobout_core_api::blob::memory_store(),
        engine: jobout_core_api::interview::default_engine(),
    })
}

async fn post_raw(app: &Router, uri: &str, cuerpo: String) -> (StatusCode, Value) {
    let respuesta = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(cuerpo))
                .unwrap(),
        )
        .await
        .unwrap();
    let estado = respuesta.status();
    let bytes = respuesta.into_body().collect().await.unwrap().to_bytes();
    (
        estado,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn post(app: &Router, uri: &str, cuerpo: Value) -> (StatusCode, Value) {
    post_raw(app, uri, cuerpo.to_string()).await
}

async fn con_token(app: &Router, uri: &str, token: &str) -> (StatusCode, Value) {
    post(app, uri, json!({ "refresh_token": token })).await
}

fn sha256_hex(texto: &str) -> String {
    Sha256::digest(texto.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Cuenta registrada; devuelve la base, la app y el refresh token del registro.
async fn con_sesion() -> (TestDb, Router, String) {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let (estado, cuerpo) = post(
        &app,
        "/api/v1/auth/register",
        json!({"email": EMAIL, "password": CLAVE, "first_name": "Ana", "last_name": "López"}),
    )
    .await;
    assert_eq!(estado, StatusCode::CREATED);
    let refresh = cuerpo["refresh_token"].as_str().unwrap().to_string();
    (db, app, refresh)
}

async fn login(app: &Router) -> String {
    let (estado, cuerpo) = post(
        app,
        "/api/v1/auth/login",
        json!({"email": EMAIL, "password": CLAVE}),
    )
    .await;
    assert_eq!(estado, StatusCode::OK);
    cuerpo["refresh_token"].as_str().unwrap().to_string()
}

async fn vivos(db: &TestDb) -> i64 {
    db.count("SELECT count(*) FROM refresh_tokens WHERE revoked_at IS NULL AND expires_at > now()")
        .await
}

// ----------------------------------------------------------------- refresh

/// R12: un refresh válido entrega tokens nuevos y el anterior queda revocado.
#[tokio::test]
async fn r12_refresh_valido_rota_el_token() {
    let (db, app, viejo) = con_sesion().await;
    let (estado, cuerpo) = con_token(&app, REFRESH, &viejo).await;

    assert_eq!(estado, StatusCode::OK, "cuerpo: {cuerpo}");
    let nuevo = cuerpo["refresh_token"].as_str().expect("refresh_token");
    assert_ne!(nuevo, viejo);
    assert!(jwt()
        .validate(cuerpo["access_token"].as_str().unwrap())
        .is_ok());
    assert_eq!(cuerpo["token_type"], "Bearer");
    assert_eq!(cuerpo["expires_in"], 900);

    // El viejo quedó revocado, el nuevo vive y se guardó solo su hash.
    assert_eq!(
        db.count(&format!(
            "SELECT count(*) FROM refresh_tokens WHERE token_hash = decode('{}', 'hex') \
             AND revoked_at IS NOT NULL",
            sha256_hex(&viejo)
        ))
        .await,
        1
    );
    assert_eq!(
        db.count(&format!(
            "SELECT count(*) FROM refresh_tokens WHERE token_hash = decode('{}', 'hex') \
             AND revoked_at IS NULL AND expires_at > now()",
            sha256_hex(nuevo)
        ))
        .await,
        1
    );
    assert_eq!(
        db.count(&format!(
            "SELECT count(*) FROM refresh_tokens WHERE position('{nuevo}'::bytea in token_hash) > 0"
        ))
        .await,
        0
    );
    // El nuevo también sirve para renovar otra vez.
    let (estado, _) = con_token(&app, REFRESH, nuevo).await;
    assert_eq!(estado, StatusCode::OK);
    db.cleanup().await;
}

/// R13: reusar un refresh ya rotado da 401 y revoca todos los refresh vivos del usuario.
#[tokio::test]
async fn r13_reuso_revoca_todas_las_sesiones() {
    let (db, app, viejo) = con_sesion().await;
    let otra_sesion = login(&app).await;
    let (_, rotado) = con_token(&app, REFRESH, &viejo).await;
    let nuevo = rotado["refresh_token"].as_str().unwrap();
    assert_eq!(vivos(&db).await, 2);

    let (estado, cuerpo) = con_token(&app, REFRESH, &viejo).await;
    assert_eq!(estado, StatusCode::UNAUTHORIZED);
    assert!(cuerpo.get("access_token").is_none());
    assert_eq!(vivos(&db).await, 0, "quedaron refresh vivos tras el reuso");

    // Ni el sucesor ni la otra sesión sirven ya.
    for token in [nuevo, otra_sesion.as_str()] {
        let (estado, _) = con_token(&app, REFRESH, token).await;
        assert_eq!(estado, StatusCode::UNAUTHORIZED);
    }
    db.cleanup().await;
}

/// R13: el reuso de un usuario no toca las sesiones de otro.
#[tokio::test]
async fn r13_el_reuso_no_afecta_a_otros_usuarios() {
    let (db, app, viejo) = con_sesion().await;
    let (_, otro) = post(
        &app,
        "/api/v1/auth/register",
        json!({"email": "luis@correo-prueba.com", "password": CLAVE,
               "first_name": "Luis", "last_name": "Paz"}),
    )
    .await;
    con_token(&app, REFRESH, &viejo).await;
    con_token(&app, REFRESH, &viejo).await;

    let (estado, _) = con_token(&app, REFRESH, otro["refresh_token"].as_str().unwrap()).await;
    assert_eq!(estado, StatusCode::OK);
    db.cleanup().await;
}

/// R13: dos refresh simultáneos con el mismo token: uno gana, el otro cuenta como reuso y
/// no queda ningún refresh vivo.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r13_refresh_simultaneos_uno_gana_y_el_otro_es_reuso() {
    for _ in 0..5 {
        let (db, app, viejo) = con_sesion().await;
        let (a, b) = tokio::join!(
            con_token(&app, REFRESH, &viejo),
            con_token(&app, REFRESH, &viejo)
        );
        let mut estados = [a.0, b.0];
        estados.sort();
        assert_eq!(estados, [StatusCode::OK, StatusCode::UNAUTHORIZED]);
        assert_eq!(vivos(&db).await, 0, "el perdedor no revocó al ganador");
        db.cleanup().await;
    }
}

/// R14: expirado y desconocido dan la misma respuesta 401, sin distinguir causas.
#[tokio::test]
async fn r14_expirado_y_desconocido_son_indistinguibles() {
    let (db, app, refresh) = con_sesion().await;
    db.exec("UPDATE refresh_tokens SET expires_at = now() - interval '1 minute'")
        .await
        .unwrap();

    let expirado = con_token(&app, REFRESH, &refresh).await;
    let desconocido = con_token(&app, REFRESH, "no-existe-este-token").await;
    let vacio = con_token(&app, REFRESH, "").await;

    assert_eq!(expirado.0, StatusCode::UNAUTHORIZED);
    assert_eq!(expirado, desconocido);
    assert_eq!(expirado, vacio);
    assert!(expirado.1.get("access_token").is_none());
    // Un expirado no se rota: sigue sin revocar.
    assert_eq!(
        db.count("SELECT count(*) FROM refresh_tokens WHERE revoked_at IS NOT NULL")
            .await,
        0
    );
    db.cleanup().await;
}

/// R14: un token desconocido no revoca nada de nadie.
#[tokio::test]
async fn r14_desconocido_no_revoca_nada() {
    let (db, app, _) = con_sesion().await;
    con_token(&app, REFRESH, "no-existe-este-token").await;
    assert_eq!(vivos(&db).await, 1);
    db.cleanup().await;
}

/// R14: un usuario suspendido recibe 403 (igual que en login) y no obtiene tokens.
#[tokio::test]
async fn r14_usuario_suspendido_recibe_403() {
    let (db, app, refresh) = con_sesion().await;
    db.exec("UPDATE users SET status = 'suspended'")
        .await
        .unwrap();
    let (estado, cuerpo) = con_token(&app, REFRESH, &refresh).await;
    assert_eq!(estado, StatusCode::FORBIDDEN);
    assert!(cuerpo.get("access_token").is_none());
    assert!(cuerpo.get("refresh_token").is_none());
    db.cleanup().await;
}

/// R14: un usuario borrado (borrado lógico) recibe 401 genérico.
#[tokio::test]
async fn r14_usuario_borrado_recibe_401() {
    let (db, app, refresh) = con_sesion().await;
    db.exec("UPDATE users SET deleted_at = now()")
        .await
        .unwrap();
    let (estado, cuerpo) = con_token(&app, REFRESH, &refresh).await;
    assert_eq!(estado, StatusCode::UNAUTHORIZED);
    assert!(cuerpo.get("access_token").is_none());
    db.cleanup().await;
}

// ------------------------------------------------------------------ logout

/// R15: logout responde 204, revoca ese refresh y usarlo después da 401.
#[tokio::test]
async fn r15_logout_revoca_el_refresh() {
    let (db, app, refresh) = con_sesion().await;
    let (estado, cuerpo) = con_token(&app, LOGOUT, &refresh).await;
    assert_eq!(estado, StatusCode::NO_CONTENT);
    assert_eq!(cuerpo, Value::Null);
    assert_eq!(vivos(&db).await, 0);

    let (estado, _) = con_token(&app, REFRESH, &refresh).await;
    assert_eq!(estado, StatusCode::UNAUTHORIZED);
    db.cleanup().await;
}

/// R15: logout es idempotente y no revela nada: desconocido y ya revocado también dan 204.
#[tokio::test]
async fn r15_logout_es_idempotente() {
    let (db, app, refresh) = con_sesion().await;
    for token in [refresh.as_str(), refresh.as_str(), "no-existe", ""] {
        let (estado, cuerpo) = con_token(&app, LOGOUT, token).await;
        assert_eq!(estado, StatusCode::NO_CONTENT, "token {token:?}");
        assert_eq!(cuerpo, Value::Null);
    }
    db.cleanup().await;
}

/// R15: logout revoca solo esa sesión, no las demás del usuario.
#[tokio::test]
async fn r15_logout_no_toca_otras_sesiones() {
    let (db, app, primera) = con_sesion().await;
    let segunda = login(&app).await;
    con_token(&app, LOGOUT, &primera).await;

    assert_eq!(vivos(&db).await, 1);
    let (estado, _) = con_token(&app, REFRESH, &segunda).await;
    assert_eq!(estado, StatusCode::OK);
    db.cleanup().await;
}

// ------------------------------------------------------------- seguridad

#[derive(Clone, Default)]
struct Registro(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Registro {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Registro {
    fn texto(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

fn capturar_log() -> (Registro, tracing::subscriber::DefaultGuard) {
    let registro = Registro::default();
    let escritor = registro.clone();
    let guarda = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            // El filtro de producción al máximo (RUST_LOG=trace): es lo que hay que garantizar.
            .with_env_filter(jobout_core_api::telemetry::env_filter(Some("trace")))
            .with_ansi(false)
            .with_writer(move || escritor.clone())
            .finish(),
    );
    (registro, guarda)
}

/// S4: el log de refresh y logout (incluido un reuso) no contiene tokens, email ni claves.
#[tokio::test]
async fn s4_el_log_de_refresh_y_logout_no_contiene_datos_sensibles() {
    let (log, _guarda) = capturar_log();
    let (db, app, primero) = con_sesion().await;
    let (_, segundo) = con_token(&app, REFRESH, &primero).await;
    let (_, reuso) = con_token(&app, REFRESH, &primero).await;
    let otro = login(&app).await;
    con_token(&app, LOGOUT, &otro).await;

    let texto = log.texto();
    assert!(texto.contains(REFRESH), "no se capturó el log de la ruta");
    let local = EMAIL.split('@').next().unwrap();
    let segundo_acceso = segundo["access_token"].as_str().unwrap();
    let segundo_refresh = segundo["refresh_token"].as_str().unwrap();
    for secreto in [
        EMAIL,
        local,
        CLAVE,
        primero.as_str(),
        otro.as_str(),
        segundo_acceso,
        segundo_refresh,
    ] {
        assert!(!texto.contains(secreto), "el log contiene {secreto:?}");
    }
    assert!(!reuso.to_string().contains(&primero));
    db.cleanup().await;
}

/// S3: un fallo de base provocado en refresh y en logout da 500 sin filtrar nada, y el
/// refresh no deja nada a medias.
#[tokio::test]
async fn s3_los_errores_de_refresh_y_logout_no_filtran() {
    let (log, _guarda) = capturar_log();
    let (db, app, refresh) = con_sesion().await;
    db.exec("ALTER TABLE refresh_tokens RENAME TO refresh_tokens_rota")
        .await
        .unwrap();

    let (e1, roto1) = con_token(&app, REFRESH, &refresh).await;
    let (e2, roto2) = con_token(&app, LOGOUT, &refresh).await;
    assert_eq!(e1, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(e2, StatusCode::INTERNAL_SERVER_ERROR);

    let texto = log.texto();
    for fuente in [roto1.to_string(), roto2.to_string(), texto] {
        for secreto in [
            EMAIL,
            CLAVE,
            refresh.as_str(),
            "does not exist",
            "relation \"refresh_tokens",
        ] {
            assert!(!fuente.contains(secreto), "se filtró {secreto:?}: {fuente}");
        }
    }
    db.cleanup().await;
}

/// S6: cuerpos de más de 8 KB se rechazan con 413 en ambas rutas.
#[tokio::test]
async fn s6_cuerpos_enormes_se_rechazan() {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let grande = json!({"refresh_token": "x".repeat(9 * 1024)}).to_string();
    for uri in [REFRESH, LOGOUT] {
        let (estado, _) = post_raw(&app, uri, grande.clone()).await;
        assert_eq!(estado, StatusCode::PAYLOAD_TOO_LARGE, "{uri}");
    }
    db.cleanup().await;
}

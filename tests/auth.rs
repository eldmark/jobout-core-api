//! Casos de verdad de registro e ingreso (R1–R11, R16, R17, S3, S4, S6 de
//! docs/working/fase1-identidad.md). Contra PostgreSQL real; cada prueba crea su propia base.
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

fn registro(email: &str, clave: &str) -> Value {
    json!({"email": email, "password": clave, "first_name": "Ana", "last_name": "López"})
}

async fn post(app: &Router, uri: &str, cuerpo: Value) -> (StatusCode, Value) {
    post_raw(app, uri, cuerpo.to_string()).await
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
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (estado, json)
}

fn sha256(texto: &str) -> String {
    Sha256::digest(texto.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ---------------------------------------------------------------- registro

/// R1: registro válido crea todo lo que debe y entrega tokens.
#[tokio::test]
async fn r1_registro_valido_crea_cuenta_completa() {
    let db = TestDb::migrated().await;
    let (estado, cuerpo) = post(&app(&db), "/api/v1/auth/register", registro(EMAIL, CLAVE)).await;

    assert_eq!(estado, StatusCode::CREATED, "cuerpo: {cuerpo}");
    let acceso = cuerpo["access_token"].as_str().expect("access_token");
    assert!(cuerpo["refresh_token"].as_str().is_some());

    assert_eq!(db.count("SELECT count(*) FROM users").await, 1);
    assert_eq!(db.count("SELECT count(*) FROM user_credentials").await, 1);
    assert_eq!(db.count("SELECT count(*) FROM applicants").await, 1);
    assert_eq!(
        db.count(
            "SELECT count(*) FROM user_roles ur JOIN roles r ON r.id = ur.role_id \
             WHERE r.code = 'applicant'"
        )
        .await,
        1
    );
    assert_eq!(
        db.count(
            "SELECT count(*) FROM user_subscriptions s JOIN plans p ON p.id = s.plan_id \
             WHERE p.code = 'free' AND s.status = 'active' AND s.source = 'free'"
        )
        .await,
        1
    );
    assert_eq!(
        db.count("SELECT count(*) FROM users WHERE status = 'active'")
            .await,
        1
    );

    // El token de acceso es válido y su `sub` es el id del usuario.
    let sub = jwt().validate(acceso).expect("token válido");
    assert_eq!(
        db.count(&format!("SELECT count(*) FROM users WHERE id = '{sub}'"))
            .await,
        1
    );
    db.cleanup().await;
}

/// R2: email repetido → 409 genérico, sin crear nada ni repetir el email.
#[tokio::test]
async fn r2_email_repetido_da_409_generico() {
    let db = TestDb::migrated().await;
    let app = app(&db);
    post(&app, "/api/v1/auth/register", registro(EMAIL, CLAVE)).await;

    let (estado, cuerpo) = post(&app, "/api/v1/auth/register", registro(EMAIL, CLAVE)).await;
    assert_eq!(estado, StatusCode::CONFLICT);
    assert!(!cuerpo.to_string().contains(EMAIL));
    assert_eq!(db.count("SELECT count(*) FROM users").await, 1);
    db.cleanup().await;
}

/// R3: la comparación de email ignora mayúsculas y espacios.
#[tokio::test]
async fn r3_email_se_normaliza() {
    let db = TestDb::migrated().await;
    let app = app(&db);
    post(&app, "/api/v1/auth/register", registro(EMAIL, CLAVE)).await;

    let otro = format!("  {}  ", EMAIL.to_uppercase());
    let (estado, _) = post(&app, "/api/v1/auth/register", registro(&otro, CLAVE)).await;
    assert_eq!(estado, StatusCode::CONFLICT);
    assert_eq!(db.count("SELECT count(*) FROM users").await, 1);
    db.cleanup().await;
}

/// R4: datos inválidos → 422, sin crear nada y sin repetir la contraseña.
#[tokio::test]
async fn r4_datos_invalidos_dan_422() {
    let db = TestDb::migrated().await;
    let app = app(&db);
    for (email, clave) in [
        ("no-es-un-email", CLAVE),
        ("sin@punto", CLAVE),
        ("con espacio@mail.com", CLAVE),
        ("", CLAVE),
        (EMAIL, "corta"),
        (EMAIL, ""),
    ] {
        let (estado, cuerpo) = post(&app, "/api/v1/auth/register", registro(email, clave)).await;
        assert_eq!(
            estado,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{email:?} / {clave:?}"
        );
        if !clave.is_empty() {
            assert!(!cuerpo.to_string().contains(clave));
        }
    }
    let mut sin_nombre = registro(EMAIL, CLAVE);
    sin_nombre["first_name"] = json!("   ");
    let (estado, _) = post(&app, "/api/v1/auth/register", sin_nombre).await;
    assert_eq!(estado, StatusCode::UNPROCESSABLE_ENTITY);

    assert_eq!(db.count("SELECT count(*) FROM users").await, 0);
    db.cleanup().await;
}

/// R5: en la base, el email está cifrado y la contraseña hasheada.
#[tokio::test]
async fn r5_nada_sensible_en_claro_en_la_base() {
    let db = TestDb::migrated().await;
    post(&app(&db), "/api/v1/auth/register", registro(EMAIL, CLAVE)).await;

    assert_eq!(
        db.count(&format!(
            "SELECT count(*) FROM users WHERE position('{EMAIL}'::bytea in email) > 0"
        ))
        .await,
        0,
        "el email quedó en claro"
    );
    assert_eq!(
        db.count("SELECT count(*) FROM user_credentials WHERE password_hash LIKE '$argon2id$%'")
            .await,
        1
    );
    assert_eq!(
        db.count(&format!(
            "SELECT count(*) FROM user_credentials WHERE position('{CLAVE}' in password_hash) > 0"
        ))
        .await,
        0
    );
    // El hash de búsqueda es el que calcula PiiKeys.
    let hex: String = pii()
        .email_hash(EMAIL)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(
        db.count(&format!(
            "SELECT count(*) FROM users WHERE email_hash = decode('{hex}', 'hex')"
        ))
        .await,
        1
    );
    db.cleanup().await;
}

/// R6: dos registros simultáneos con el mismo email → uno 201, otro 409, una sola fila.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r6_registros_simultaneos_dejan_una_sola_cuenta() {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let (a, b) = tokio::join!(
        post(&app, "/api/v1/auth/register", registro(EMAIL, CLAVE)),
        post(&app, "/api/v1/auth/register", registro(EMAIL, CLAVE)),
    );
    let mut estados = [a.0, b.0];
    estados.sort();
    assert_eq!(estados, [StatusCode::CREATED, StatusCode::CONFLICT]);
    assert_eq!(db.count("SELECT count(*) FROM users").await, 1);
    assert_eq!(db.count("SELECT count(*) FROM applicants").await, 1);
    db.cleanup().await;
}

// ------------------------------------------------------------------- login

async fn con_cuenta() -> (TestDb, Router) {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let (estado, _) = post(&app, "/api/v1/auth/register", registro(EMAIL, CLAVE)).await;
    assert_eq!(estado, StatusCode::CREATED);
    (db, app)
}

fn credenciales(email: &str, clave: &str) -> Value {
    json!({"email": email, "password": clave})
}

/// R7: ingreso correcto → 200 con tokens válidos.
#[tokio::test]
async fn r7_login_correcto_entrega_tokens() {
    let (db, app) = con_cuenta().await;
    let (estado, cuerpo) = post(&app, "/api/v1/auth/login", credenciales(EMAIL, CLAVE)).await;

    assert_eq!(estado, StatusCode::OK, "cuerpo: {cuerpo}");
    assert!(jwt()
        .validate(cuerpo["access_token"].as_str().unwrap())
        .is_ok());
    assert!(cuerpo["refresh_token"].as_str().is_some());
    assert_eq!(cuerpo["token_type"], "Bearer");
    assert_eq!(cuerpo["expires_in"], 900);
    // También entra con el email escrito distinto.
    let (estado, _) = post(
        &app,
        "/api/v1/auth/login",
        credenciales(&EMAIL.to_uppercase(), CLAVE),
    )
    .await;
    assert_eq!(estado, StatusCode::OK);
    db.cleanup().await;
}

/// R8: contraseña incorrecta y email inexistente son indistinguibles.
#[tokio::test]
async fn r8_fallos_de_login_son_indistinguibles() {
    let (db, app) = con_cuenta().await;
    let mala = post(
        &app,
        "/api/v1/auth/login",
        credenciales(EMAIL, "otra-clave-larga-1"),
    )
    .await;
    let inexistente = post(
        &app,
        "/api/v1/auth/login",
        credenciales("nadie@correo-prueba.com", "otra-clave-larga-1"),
    )
    .await;

    assert_eq!(mala.0, StatusCode::UNAUTHORIZED);
    assert_eq!(mala, inexistente);
    db.cleanup().await;
}

/// R10: cinco fallos seguidos bloquean, incluso con la contraseña correcta.
#[tokio::test]
async fn r10_cinco_fallos_bloquean_el_login() {
    let (db, app) = con_cuenta().await;
    for _ in 0..5 {
        let (estado, _) = post(
            &app,
            "/api/v1/auth/login",
            credenciales(EMAIL, "mala-clave-larga-1"),
        )
        .await;
        assert_eq!(estado, StatusCode::UNAUTHORIZED);
    }
    let (estado, _) = post(&app, "/api/v1/auth/login", credenciales(EMAIL, CLAVE)).await;
    assert_eq!(estado, StatusCode::TOO_MANY_REQUESTS);

    // El bloqueo es por email: otra cuenta no se ve afectada.
    let (estado, _) = post(
        &app,
        "/api/v1/auth/register",
        registro("luis@correo-prueba.com", CLAVE),
    )
    .await;
    assert_eq!(estado, StatusCode::CREATED);
    let (estado, _) = post(
        &app,
        "/api/v1/auth/login",
        credenciales("luis@correo-prueba.com", CLAVE),
    )
    .await;
    assert_eq!(estado, StatusCode::OK);
    db.cleanup().await;
}

/// R10: el bloqueo también aplica a emails que no existen (sin revelar cuáles existen).
#[tokio::test]
async fn r10_el_bloqueo_no_revela_si_el_email_existe() {
    let (db, app) = con_cuenta().await;
    let fantasma = credenciales("fantasma@correo-prueba.com", "mala-clave-larga-1");
    for _ in 0..5 {
        post(&app, "/api/v1/auth/login", fantasma.clone()).await;
    }
    let (estado, _) = post(&app, "/api/v1/auth/login", fantasma).await;
    assert_eq!(estado, StatusCode::TOO_MANY_REQUESTS);
    db.cleanup().await;
}

/// R11: un ingreso correcto reinicia el contador.
#[tokio::test]
async fn r11_login_correcto_reinicia_el_contador() {
    let (db, app) = con_cuenta().await;
    let mala = credenciales(EMAIL, "mala-clave-larga-1");
    for _ in 0..4 {
        post(&app, "/api/v1/auth/login", mala.clone()).await;
    }
    let (estado, _) = post(&app, "/api/v1/auth/login", credenciales(EMAIL, CLAVE)).await;
    assert_eq!(estado, StatusCode::OK);
    for _ in 0..4 {
        let (estado, _) = post(&app, "/api/v1/auth/login", mala.clone()).await;
        assert_eq!(estado, StatusCode::UNAUTHORIZED);
    }
    let (estado, _) = post(&app, "/api/v1/auth/login", credenciales(EMAIL, CLAVE)).await;
    assert_eq!(estado, StatusCode::OK, "el contador no se reinició");
    db.cleanup().await;
}

/// R10: el bloqueo vence al pasar la ventana.
#[tokio::test]
async fn r10_el_bloqueo_vence_con_la_ventana() {
    let (db, app) = con_cuenta().await;
    for _ in 0..5 {
        post(
            &app,
            "/api/v1/auth/login",
            credenciales(EMAIL, "mala-clave-larga-1"),
        )
        .await;
    }
    db.exec("UPDATE login_attempts SET window_started_at = now() - interval '16 minutes'")
        .await
        .unwrap();
    let (estado, _) = post(&app, "/api/v1/auth/login", credenciales(EMAIL, CLAVE)).await;
    assert_eq!(estado, StatusCode::OK);
    db.cleanup().await;
}

/// R16: el refresh token se guarda solo como hash.
#[tokio::test]
async fn r16_el_refresh_token_se_guarda_como_hash() {
    let (db, app) = con_cuenta().await;
    let (_, cuerpo) = post(&app, "/api/v1/auth/login", credenciales(EMAIL, CLAVE)).await;
    let refresh = cuerpo["refresh_token"].as_str().unwrap();

    assert_eq!(
        db.count(&format!(
            "SELECT count(*) FROM refresh_tokens WHERE position('{refresh}'::bytea in token_hash) > 0"
        ))
        .await,
        0,
        "el token quedó en claro"
    );
    assert_eq!(
        db.count(&format!(
            "SELECT count(*) FROM refresh_tokens WHERE token_hash = decode('{}', 'hex') \
             AND expires_at > now() AND revoked_at IS NULL",
            sha256(refresh)
        ))
        .await,
        1
    );
    db.cleanup().await;
}

/// R17: un usuario suspendido no entra.
#[tokio::test]
async fn r17_usuario_suspendido_recibe_403() {
    let (db, app) = con_cuenta().await;
    db.exec("UPDATE users SET status = 'suspended'")
        .await
        .unwrap();
    let (estado, cuerpo) = post(&app, "/api/v1/auth/login", credenciales(EMAIL, CLAVE)).await;
    assert_eq!(estado, StatusCode::FORBIDDEN);
    assert!(cuerpo.get("access_token").is_none());
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

/// S4: el log de peticiones correctas no contiene email, contraseña ni tokens.
#[tokio::test]
async fn s4_el_log_no_contiene_datos_sensibles() {
    let (log, _guarda) = capturar_log();
    let db = TestDb::migrated().await;
    let app = app(&db);
    let (_, registrado) = post(&app, "/api/v1/auth/register", registro(EMAIL, CLAVE)).await;
    let (_, ingreso) = post(&app, "/api/v1/auth/login", credenciales(EMAIL, CLAVE)).await;

    let texto = log.texto();
    assert!(!texto.is_empty(), "no se capturó ningún log");
    let local = EMAIL.split('@').next().unwrap();
    for secreto in [
        EMAIL,
        local,
        CLAVE,
        registrado["access_token"].as_str().unwrap(),
        registrado["refresh_token"].as_str().unwrap(),
        ingreso["access_token"].as_str().unwrap(),
        ingreso["refresh_token"].as_str().unwrap(),
    ] {
        assert!(!texto.contains(secreto), "el log contiene {secreto:?}");
    }
    db.cleanup().await;
}

/// S3: un error provocado no filtra email ni contraseña, ni en la respuesta ni en el log.
#[tokio::test]
async fn s3_los_errores_no_filtran_datos_sensibles() {
    let (log, _guarda) = capturar_log();
    let db = TestDb::migrated().await;
    let app = app(&db);
    post(&app, "/api/v1/auth/register", registro(EMAIL, CLAVE)).await;

    // Error de negocio: email repetido.
    let (_, duplicado) = post(&app, "/api/v1/auth/register", registro(EMAIL, CLAVE)).await;
    // Error de infraestructura: se borra una tabla y el registro de otro email revienta.
    db.exec("DROP TABLE user_subscriptions").await.unwrap();
    let otro = "otra.persona@correo-prueba.com";
    let (estado, roto) = post(&app, "/api/v1/auth/register", registro(otro, CLAVE)).await;
    assert_eq!(estado, StatusCode::INTERNAL_SERVER_ERROR);
    // Y nada quedó a medias.
    assert_eq!(db.count("SELECT count(*) FROM users").await, 1);

    let texto = log.texto();
    for fuente in [duplicado.to_string(), roto.to_string(), texto] {
        for secreto in [EMAIL, otro, CLAVE] {
            assert!(!fuente.contains(secreto), "se filtró {secreto:?}: {fuente}");
        }
    }
    db.cleanup().await;
}

/// S6: un cuerpo de más de 8 KB se rechaza con 413.
#[tokio::test]
async fn s6_cuerpos_enormes_se_rechazan() {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let grande = json!({"email": EMAIL, "password": "x".repeat(9 * 1024)}).to_string();
    let (estado, _) = post_raw(&app, "/api/v1/auth/login", grande.clone()).await;
    assert_eq!(estado, StatusCode::PAYLOAD_TOO_LARGE);
    let (estado, _) = post_raw(&app, "/api/v1/auth/register", grande).await;
    assert_eq!(estado, StatusCode::PAYLOAD_TOO_LARGE);
    db.cleanup().await;
}

/// R18: /health sigue pública y sin estado.
#[tokio::test]
async fn r18_health_sigue_publico() {
    let db = TestDb::migrated().await;
    let respuesta = app(&db)
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(respuesta.status(), StatusCode::OK);
    db.cleanup().await;
}

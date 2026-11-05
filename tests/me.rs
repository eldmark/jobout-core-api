//! Casos de verdad de `AuthUser`, `GET /api/v1/me` y `DELETE /api/v1/me` (M1, M2, B1–B7, S1–S4,
//! R17 de docs/working/fase1-identidad.md). Contra PostgreSQL real; una base por prueba.
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
use tower::ServiceExt;

const EMAIL: &str = "ana.lopez@correo-prueba.com";
const OTRO_EMAIL: &str = "luis.perez@correo-prueba.com";
const CLAVE: &str = "una-clave-larga-123";
const ME: &str = "/api/v1/me";

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
    encabezado: Option<&str>,
    cuerpo: Option<Value>,
) -> (StatusCode, Value) {
    let mut peticion = Request::builder().method(metodo).uri(uri);
    if let Some(valor) = encabezado {
        peticion = peticion.header("authorization", valor);
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

async fn post(app: &Router, uri: &str, cuerpo: Value) -> (StatusCode, Value) {
    enviar(app, "POST", uri, None, Some(cuerpo)).await
}

async fn get_me(app: &Router, token: &str) -> (StatusCode, Value) {
    enviar(app, "GET", ME, Some(&format!("Bearer {token}")), None).await
}

async fn delete_me(app: &Router, token: &str, clave: &str) -> (StatusCode, Value) {
    enviar(
        app,
        "DELETE",
        ME,
        Some(&format!("Bearer {token}")),
        Some(json!({"password": clave})),
    )
    .await
}

/// Registra una cuenta y devuelve su token de acceso.
async fn registrar(app: &Router, email: &str, nombre: &str) -> String {
    let (estado, cuerpo) = post(
        app,
        "/api/v1/auth/register",
        json!({"email": email, "password": CLAVE, "first_name": nombre, "last_name": "Prueba"}),
    )
    .await;
    assert_eq!(estado, StatusCode::CREATED, "{cuerpo}");
    cuerpo["access_token"].as_str().unwrap().to_string()
}

async fn con_cuenta() -> (TestDb, Router, String) {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let token = registrar(&app, EMAIL, "Ana").await;
    (db, app, token)
}

// ------------------------------------------------------------------ GET /me

/// M1: token válido → 200 con los datos de la cuenta, el plan activo y el email descifrado.
#[tokio::test]
async fn m1_me_devuelve_la_cuenta() {
    let (db, app, token) = con_cuenta().await;
    let (estado, cuerpo) = get_me(&app, &token).await;

    assert_eq!(estado, StatusCode::OK, "{cuerpo}");
    assert_eq!(cuerpo["email"], EMAIL);
    assert_eq!(cuerpo["first_name"], "Ana");
    assert_eq!(cuerpo["last_name"], "Prueba");
    assert_eq!(cuerpo["status"], "active");
    assert_eq!(cuerpo["plan"]["code"], "free");
    assert!(cuerpo["plan"]["name"]
        .as_str()
        .is_some_and(|n| !n.is_empty()));
    assert_eq!(cuerpo["id"], jwt().validate(&token).unwrap());
    db.cleanup().await;
}

/// M2: /me solo puede traer un conjunto fijo de campos; nada interno.
#[tokio::test]
async fn m2_me_no_expone_campos_internos() {
    let (db, app, token) = con_cuenta().await;
    let (_, cuerpo) = get_me(&app, &token).await;

    let permitidos = ["id", "first_name", "last_name", "status", "email", "plan"];
    let campos: Vec<&String> = cuerpo.as_object().unwrap().keys().collect();
    for campo in &campos {
        assert!(
            permitidos.contains(&campo.as_str()),
            "/me devolvió el campo inesperado '{campo}'"
        );
    }
    assert_eq!(campos.len(), permitidos.len(), "faltan campos: {campos:?}");
    let plan: Vec<&String> = cuerpo["plan"].as_object().unwrap().keys().collect();
    assert_eq!(plan.len(), 2, "el plan solo trae code y name: {plan:?}");
    for prohibido in ["email_hash", "password_hash", "clerk_user_id", "deleted_at"] {
        assert!(!cuerpo.to_string().contains(prohibido));
    }
    db.cleanup().await;
}

// ---------------------------------------------------------------- AuthUser

/// S2: sin token, con esquema distinto, `Bearer` vacío o token inválido → 401 en ambas rutas.
#[tokio::test]
async fn s2_sin_credenciales_validas_da_401() {
    let (db, app, token) = con_cuenta().await;
    let otra_firma = JwtKeys::from_secret(&"x".repeat(40))
        .unwrap()
        .issue("00000000-0000-0000-0000-000000000000")
        .unwrap();
    let casos: Vec<Option<String>> = vec![
        None,
        Some(token.clone()),                  // sin esquema
        Some(format!("Basic {token}")),       // otro esquema
        Some("Bearer".to_string()),           // sin token
        Some("Bearer ".to_string()),          // token vacío
        Some("Bearer    ".to_string()),       // solo espacios
        Some("Bearer basura".to_string()),    // no es un JWT
        Some(format!("Bearer {otra_firma}")), // firmado con otra clave
    ];
    for encabezado in casos {
        for (metodo, cuerpo) in [("GET", None), ("DELETE", Some(json!({"password": CLAVE})))] {
            let (estado, resp) = enviar(&app, metodo, ME, encabezado.as_deref(), cuerpo).await;
            assert_eq!(estado, StatusCode::UNAUTHORIZED, "{metodo} {encabezado:?}");
            assert!(resp["error"].is_string() && resp["message"].is_string());
            assert!(!resp.to_string().contains(&token));
        }
    }
    assert_eq!(db.count("SELECT count(*) FROM users").await, 1);
    db.cleanup().await;
}

/// R17: un usuario suspendido con token vigente recibe 403 (no sigue usando la API).
#[tokio::test]
async fn r17_suspendido_recibe_403_en_me() {
    let (db, app, token) = con_cuenta().await;
    db.exec("UPDATE users SET status = 'suspended'")
        .await
        .unwrap();

    let (estado, cuerpo) = get_me(&app, &token).await;
    assert_eq!(estado, StatusCode::FORBIDDEN, "{cuerpo}");
    assert!(cuerpo.get("email").is_none());
    let (estado, _) = delete_me(&app, &token, CLAVE).await;
    assert_eq!(estado, StatusCode::FORBIDDEN);
    assert_eq!(db.count("SELECT count(*) FROM users").await, 1);
    db.cleanup().await;
}

/// Una cuenta marcada como borrada (`deleted_at`) tampoco entra con un token vigente.
#[tokio::test]
async fn cuenta_con_deleted_at_da_401() {
    let (db, app, token) = con_cuenta().await;
    db.exec("UPDATE users SET deleted_at = now()")
        .await
        .unwrap();
    let (estado, _) = get_me(&app, &token).await;
    assert_eq!(estado, StatusCode::UNAUTHORIZED);
    db.cleanup().await;
}

// ---------------------------------------------------------------- DELETE /me

/// B1: contraseña correcta → 204 y la cuenta desaparece de verdad, con todo lo suyo.
#[tokio::test]
async fn b1_delete_borra_la_cuenta_y_sus_datos() {
    let (db, app, token) = con_cuenta().await;
    let (estado, _) = delete_me(&app, &token, CLAVE).await;
    assert_eq!(estado, StatusCode::NO_CONTENT);

    for tabla in [
        "users",
        "applicants",
        "user_roles",
        "user_credentials",
        "refresh_tokens",
        "user_subscriptions",
    ] {
        assert_eq!(
            db.count(&format!("SELECT count(*) FROM {tabla}")).await,
            0,
            "quedaron filas en {tabla}"
        );
    }
    db.cleanup().await;
}

/// B2: contraseña incorrecta → 403 y no se borra nada.
#[tokio::test]
async fn b2_contrasena_incorrecta_no_borra() {
    let (db, app, token) = con_cuenta().await;
    let (estado, cuerpo) = delete_me(&app, &token, "otra-clave-larga-1").await;

    assert_eq!(estado, StatusCode::FORBIDDEN, "{cuerpo}");
    assert!(!cuerpo.to_string().contains("otra-clave-larga-1"));
    assert_eq!(db.count("SELECT count(*) FROM users").await, 1);
    assert_eq!(db.count("SELECT count(*) FROM privacy_events").await, 0);
    assert_eq!(get_me(&app, &token).await.0, StatusCode::OK);
    db.cleanup().await;
}

/// B2 + R10: las contraseñas erróneas cuentan en el mismo bloqueo que el login.
#[tokio::test]
async fn b2_los_intentos_fallidos_bloquean_como_en_login() {
    let (db, app, token) = con_cuenta().await;
    for _ in 0..5 {
        let (estado, _) = delete_me(&app, &token, "mala-clave-larga-1").await;
        assert_eq!(estado, StatusCode::FORBIDDEN);
    }
    // Ni la contraseña correcta pasa durante el bloqueo, y no se borró nada.
    let (estado, _) = delete_me(&app, &token, CLAVE).await;
    assert_eq!(estado, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(db.count("SELECT count(*) FROM users").await, 1);
    // El mismo bloqueo vale para el login.
    let (estado, _) = post(
        &app,
        "/api/v1/auth/login",
        json!({"email": EMAIL, "password": CLAVE}),
    )
    .await;
    assert_eq!(estado, StatusCode::TOO_MANY_REQUESTS);
    db.cleanup().await;
}

/// B3: una sola fila en `privacy_events`, con el hash del email y sin datos personales.
#[tokio::test]
async fn b3_queda_una_constancia_sin_datos_personales() {
    let (db, app, token) = con_cuenta().await;
    let id = jwt().validate(&token).unwrap();
    let hash = pii().email_hash(EMAIL);
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    delete_me(&app, &token, CLAVE).await;

    assert_eq!(db.count("SELECT count(*) FROM privacy_events").await, 1);
    assert_eq!(
        db.count(&format!(
            "SELECT count(*) FROM privacy_events WHERE kind = 'account_deleted' \
             AND subject_hash = decode('{hex}', 'hex') AND created_at > now() - interval '1 minute'"
        ))
        .await,
        1
    );
    // Toda la fila, pasada a texto, no contiene email, nombre ni el id de usuario.
    for dato in [EMAIL, "ana.lopez", "Ana", "Prueba", id.as_str()] {
        assert_eq!(
            db.count(&format!(
                "SELECT count(*) FROM privacy_events p WHERE p::text ILIKE '%{dato}%'"
            ))
            .await,
            0,
            "la constancia contiene {dato:?}"
        );
    }
    assert_eq!(
        db.count(
            "SELECT count(*) FROM privacy_events WHERE created_by IS NOT NULL OR notes IS NOT NULL"
        )
        .await,
        0
    );
    db.cleanup().await;
}

/// B4: tras borrar, el token viejo da 401 (también para repetir el DELETE) y el login falla.
#[tokio::test]
async fn b4_el_token_y_el_login_dejan_de_servir() {
    let (db, app, token) = con_cuenta().await;
    delete_me(&app, &token, CLAVE).await;

    assert_eq!(get_me(&app, &token).await.0, StatusCode::UNAUTHORIZED);
    // Desvío del plan: repetir el DELETE no es un 204 idempotente, es un 401.
    assert_eq!(
        delete_me(&app, &token, CLAVE).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (estado, _) = post(
        &app,
        "/api/v1/auth/login",
        json!({"email": EMAIL, "password": CLAVE}),
    )
    .await;
    assert_eq!(estado, StatusCode::UNAUTHORIZED);
    assert_eq!(db.count("SELECT count(*) FROM privacy_events").await, 1);
    db.cleanup().await;
}

/// B5 + S1: solo se borra al usuario del token; no hay forma de apuntar a otro.
#[tokio::test]
async fn b5_s1_solo_se_borra_al_usuario_del_token() {
    let (db, app, token_a) = con_cuenta().await;
    let token_b = registrar(&app, OTRO_EMAIL, "Luis").await;
    let id_b = jwt().validate(&token_b).unwrap();

    // /me de cada uno resuelve a su propia cuenta.
    assert_eq!(get_me(&app, &token_a).await.1["email"], EMAIL);
    assert_eq!(get_me(&app, &token_b).await.1["email"], OTRO_EMAIL);
    // No hay ruta por id.
    let (estado, _) = enviar(
        &app,
        "GET",
        &format!("{ME}/{id_b}"),
        Some(&format!("Bearer {token_a}")),
        None,
    )
    .await;
    assert_eq!(estado, StatusCode::NOT_FOUND);

    let (estado, _) = delete_me(&app, &token_a, CLAVE).await;
    assert_eq!(estado, StatusCode::NO_CONTENT);

    assert_eq!(db.count("SELECT count(*) FROM users").await, 1);
    for tabla in [
        "applicants",
        "user_roles",
        "user_credentials",
        "refresh_tokens",
        "user_subscriptions",
    ] {
        assert_eq!(
            db.count(&format!(
                "SELECT count(*) FROM {tabla} WHERE user_id = '{id_b}'"
            ))
            .await,
            1,
            "{tabla} de B"
        );
    }
    let (estado, cuerpo) = get_me(&app, &token_b).await;
    assert_eq!(estado, StatusCode::OK);
    assert_eq!(cuerpo["email"], OTRO_EMAIL);
    db.cleanup().await;
}

/// B6: si el borrado falla a mitad, no queda nada a medias (ni la constancia).
#[tokio::test]
async fn b6_si_falla_a_mitad_no_se_borra_nada() {
    let (db, app, token) = con_cuenta().await;
    // El borrado de `users` revienta. La constancia ya se escribió dentro de la transacción:
    // si no hay rollback, quedaría una fila fantasma.
    db.exec(
        "CREATE FUNCTION boom() RETURNS trigger AS $$ BEGIN RAISE EXCEPTION 'boom'; END $$ \
         LANGUAGE plpgsql; \
         CREATE TRIGGER boom BEFORE DELETE ON users FOR EACH ROW EXECUTE FUNCTION boom();",
    )
    .await
    .unwrap();

    let (estado, cuerpo) = delete_me(&app, &token, CLAVE).await;
    assert_eq!(estado, StatusCode::INTERNAL_SERVER_ERROR, "{cuerpo}");
    for tabla in [
        "users",
        "applicants",
        "user_roles",
        "user_credentials",
        "user_subscriptions",
    ] {
        assert_eq!(
            db.count(&format!("SELECT count(*) FROM {tabla}")).await,
            1,
            "{tabla} quedó a medias"
        );
    }
    assert_eq!(db.count("SELECT count(*) FROM privacy_events").await, 0);
    db.cleanup().await;
}

/// B7: tras borrar se puede registrar de nuevo el mismo email: cuenta nueva y vacía.
#[tokio::test]
async fn b7_se_puede_volver_a_registrar_el_mismo_email() {
    let (db, app, token) = con_cuenta().await;
    let id_viejo = jwt().validate(&token).unwrap();
    delete_me(&app, &token, CLAVE).await;

    let nuevo = registrar(&app, EMAIL, "Ana").await;
    let id_nuevo = jwt().validate(&nuevo).unwrap();
    assert_ne!(id_viejo, id_nuevo);
    let (estado, cuerpo) = get_me(&app, &nuevo).await;
    assert_eq!(estado, StatusCode::OK);
    assert_eq!(cuerpo["email"], EMAIL);
    assert_eq!(db.count("SELECT count(*) FROM users").await, 1);
    assert_eq!(db.count("SELECT count(*) FROM applicants").await, 1);
    // El token de la cuenta anterior sigue sin servir.
    assert_eq!(get_me(&app, &token).await.0, StatusCode::UNAUTHORIZED);
    // La constancia del borrado anterior se conserva.
    assert_eq!(db.count("SELECT count(*) FROM privacy_events").await, 1);
    db.cleanup().await;
}

/// Dos DELETE simultáneos de la misma cuenta: uno borra (204), el otro ya no la encuentra
/// (401) y la constancia no se duplica.
#[tokio::test]
async fn b3_dos_delete_simultaneos_dejan_una_sola_constancia() {
    let (db, app, token) = con_cuenta().await;
    let (a, b) = tokio::join!(
        delete_me(&app, &token, CLAVE),
        delete_me(&app, &token, CLAVE)
    );
    let mut estados = [a.0, b.0];
    estados.sort();
    assert_eq!(
        estados,
        [StatusCode::NO_CONTENT, StatusCode::UNAUTHORIZED],
        "{a:?} {b:?}"
    );
    assert_eq!(db.count("SELECT count(*) FROM privacy_events").await, 1);
    db.cleanup().await;
}

/// Un cuerpo sin contraseña no borra nada.
#[tokio::test]
async fn delete_sin_contrasena_no_borra() {
    let (db, app, token) = con_cuenta().await;
    let (estado, _) = enviar(
        &app,
        "DELETE",
        ME,
        Some(&format!("Bearer {token}")),
        Some(json!({})),
    )
    .await;
    assert_eq!(estado, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(db.count("SELECT count(*) FROM users").await, 1);
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

/// S4: el log de GET y DELETE correctos no contiene email, contraseña ni JWT.
#[tokio::test]
async fn s4_el_log_de_me_no_contiene_datos_sensibles() {
    let (log, _guarda) = capturar_log();
    let (db, app, token) = con_cuenta().await;
    let nombre = EMAIL.split('@').next().unwrap();
    assert_eq!(get_me(&app, &token).await.0, StatusCode::OK);
    assert_eq!(
        delete_me(&app, &token, CLAVE).await.0,
        StatusCode::NO_CONTENT
    );

    let texto = log.texto();
    assert!(!texto.is_empty(), "no se capturó ningún log");
    for secreto in [EMAIL, nombre, CLAVE, token.as_str()] {
        assert!(!texto.contains(secreto), "el log contiene {secreto:?}");
    }
    db.cleanup().await;
}

/// S3: errores (token inválido, contraseña mala, fallo de base) no filtran nada, ni en la
/// respuesta ni en el log.
#[tokio::test]
async fn s3_los_errores_de_me_no_filtran_datos() {
    let (log, _guarda) = capturar_log();
    let (db, app, token) = con_cuenta().await;
    let nombre = EMAIL.split('@').next().unwrap();
    let mala = "otra-clave-larga-1";
    let invalido = "Bearer abc.def.ghi";

    let mut respuestas = vec![];
    respuestas.push(enviar(&app, "GET", ME, Some(invalido), None).await);
    respuestas.push(delete_me(&app, &token, mala).await);
    // Fallo de infraestructura: sin la tabla de suscripciones, /me revienta.
    db.exec("DROP TABLE user_subscriptions").await.unwrap();
    let roto = get_me(&app, &token).await;
    assert_eq!(roto.0, StatusCode::INTERNAL_SERVER_ERROR);
    respuestas.push(roto);

    let mut fuentes: Vec<String> = respuestas.iter().map(|(_, c)| c.to_string()).collect();
    fuentes.push(log.texto());
    for fuente in fuentes {
        for secreto in [EMAIL, nombre, CLAVE, mala, token.as_str(), "abc.def.ghi"] {
            assert!(!fuente.contains(secreto), "se filtró {secreto:?}: {fuente}");
        }
    }
    db.cleanup().await;
}

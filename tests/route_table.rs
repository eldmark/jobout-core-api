//! S5: toda ruta exige autenticación, salvo una lista explícita y justificada.
//!
//! La tabla de rutas (`routes::table`) es la **única** forma de montar una ruta: cada entrada
//! declara su acceso y, si es protegida, la autenticación se aplica al montarla, aunque el
//! handler se olvide de pedirla. Estas pruebas ya no leen el código fuente: ejercitan la
//! aplicación real, ruta por ruta y método por método.
//!
//! Se escribieron antes que la tabla, en rojo. Ver CONTRIBUTING.md.

#[allow(dead_code)]
mod common;

use std::{collections::BTreeSet, sync::Arc};

use axum::{
    body::Body,
    http::{header::AUTHORIZATION, Method, Request, StatusCode},
    routing::get,
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use common::TestDb;
use http_body_util::BodyExt;
use jobout_core_api::{
    app_with_state,
    crypto::PiiKeys,
    jwt::JwtKeys,
    routes::table::{self, Access, RouteSpec},
    state::AppState,
};
use sea_orm::DatabaseConnection;
use serde_json::{json, Value};
use tower::ServiceExt;

const METODOS: [Method; 5] = [
    Method::GET,
    Method::POST,
    Method::PUT,
    Method::PATCH,
    Method::DELETE,
];

fn pii() -> Arc<PiiKeys> {
    Arc::new(
        PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
    )
}

fn jwt() -> Arc<JwtKeys> {
    Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap())
}

/// Estado sin base de datos: las pruebas de este bloque no llegan a tocarla, porque la
/// autenticación rechaza antes de consultar.
fn estado_sin_base() -> AppState {
    AppState {
        db: DatabaseConnection::Disconnected,
        pii: pii(),
        jwt: jwt(),
        blobs: jobout_core_api::blob::memory_store(),
    }
}

async fn pedir(app: &Router, metodo: &Method, ruta: &str, auth: Option<&str>) -> StatusCode {
    let mut peticion = Request::builder().method(metodo.clone()).uri(ruta);
    if let Some(a) = auth {
        peticion = peticion.header(AUTHORIZATION, a);
    }
    app.clone()
        .oneshot(peticion.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

fn metodo(nombre: &str) -> Method {
    Method::from_bytes(nombre.as_bytes()).unwrap()
}

// ------------------------------------------------------------ la tabla

/// T7: la tabla es exactamente esta. Añadir o quitar una ruta obliga a editar esta lista, y por
/// tanto a que alguien lo revise en el PR.
#[test]
fn t7_las_rutas_declaradas_son_las_esperadas() {
    let reales: BTreeSet<(String, bool)> = table::all()
        .iter()
        .map(|r| (r.path.to_string(), matches!(r.access, Access::Public(_))))
        .collect();
    let esperadas: BTreeSet<(String, bool)> = [
        ("/health", true),
        ("/api/v1/auth/register", true),
        ("/api/v1/auth/login", true),
        ("/api/v1/auth/refresh", true),
        ("/api/v1/auth/logout", true),
        ("/api/v1/me", false),
        ("/api/v1/catalogs/{nombre}", false),
        ("/api/v1/cvs", false),
        ("/api/v1/cvs/{id}", false),
        ("/api/v1/cvs/{id}/file", false),
        ("/api/v1/cvs/{id}/primary", false),
        ("/api/v1/profile", false),
        ("/api/v1/cvs/{id}/extraction", false),
        ("/api/v1/cvs/{id}/extraction/confirm", false),
        ("/api/v1/interviews", false),
        ("/api/v1/interviews/{id}", false),
        ("/api/v1/interviews/{id}/pause", false),
        ("/api/v1/interviews/{id}/resume", false),
        ("/api/v1/interviews/{id}/abandon", false),
    ]
    .into_iter()
    .map(|(p, publica)| (p.to_string(), publica))
    .collect();
    assert_eq!(reales, esperadas);
}

/// T1: toda ruta pública explica por qué lo es, de verdad (no un «ok»).
#[test]
fn t1_toda_ruta_publica_tiene_una_justificacion_real() {
    for r in table::all() {
        assert!(
            r.path.starts_with('/'),
            "ruta sin barra inicial: {}",
            r.path
        );
        if let Access::Public(razon) = r.access {
            assert!(
                razon.trim().chars().count() >= 20,
                "{} es pública con una justificación demasiado corta: {razon:?}",
                r.path
            );
        }
    }
}

/// T2: ninguna combinación ruta+método se declara dos veces.
#[test]
fn t2_no_hay_rutas_duplicadas() {
    let mut vistas = BTreeSet::new();
    for r in table::all() {
        assert!(!r.methods.is_empty(), "{} sin métodos declarados", r.path);
        for m in r.methods {
            assert!(
                vistas.insert((r.path, *m)),
                "{m} {} está declarada dos veces",
                r.path
            );
        }
    }
}

// ------------------------------------------------- la aplicación real

/// T3: cada ruta protegida rechaza con 401 sin token, con esquema distinto, con `Bearer` vacío
/// y con basura, **en cada método declarado**.
#[tokio::test]
async fn t3_las_rutas_protegidas_exigen_token() {
    let app = app_with_state(estado_sin_base());
    let mut probadas = 0;
    for r in table::all()
        .iter()
        .filter(|r| matches!(r.access, Access::Authenticated))
    {
        for m in r.methods {
            let m = metodo(m);
            for auth in [
                None,
                Some("Basic abc"),
                Some("Bearer"),
                Some("Bearer "),
                Some("Bearer esto.no.es-un-jwt"),
            ] {
                assert_eq!(
                    pedir(&app, &m, r.path, auth).await,
                    StatusCode::UNAUTHORIZED,
                    "{m} {} con {auth:?} no dio 401",
                    r.path
                );
            }
            probadas += 1;
        }
    }
    assert!(probadas >= 2, "no se probó ninguna ruta protegida");
}

/// T3b: un JWT bien formado pero firmado con **otra** clave tampoco pasa.
#[tokio::test]
async fn t3b_un_jwt_de_otra_clave_no_pasa() {
    let app = app_with_state(estado_sin_base());
    let ajeno = JwtKeys::from_secret(&"x".repeat(40)).unwrap();
    let token = ajeno.issue("00000000-0000-7000-8000-000000000000").unwrap();
    for r in table::all()
        .iter()
        .filter(|r| matches!(r.access, Access::Authenticated))
    {
        for m in r.methods {
            assert_eq!(
                pedir(&app, &metodo(m), r.path, Some(&format!("Bearer {token}"))).await,
                StatusCode::UNAUTHORIZED
            );
        }
    }
}

/// T4: los métodos declarados son los que existen de verdad. Lo declarado no da 405; lo no
/// declarado sí. Así la tabla no puede mentir sobre qué atiende cada ruta.
#[tokio::test]
async fn t4_los_metodos_declarados_son_los_reales() {
    let app = app_with_state(estado_sin_base());
    for r in table::all() {
        for m in METODOS {
            let declarado = r.methods.contains(&m.as_str());
            let estado = pedir(&app, &m, r.path, None).await;
            if declarado {
                assert_ne!(estado, StatusCode::METHOD_NOT_ALLOWED, "{m} {}", r.path);
            } else {
                assert_eq!(
                    estado,
                    StatusCode::METHOD_NOT_ALLOWED,
                    "{m} {} responde pero no está declarado",
                    r.path
                );
            }
        }
    }
}

/// T6: una ruta que no existe es 404 para todos, sin token y con él: no revela nada.
#[tokio::test]
async fn t6_una_ruta_inexistente_es_404() {
    let app = app_with_state(estado_sin_base());
    assert_eq!(
        pedir(&app, &Method::GET, "/api/v1/no-existe", None).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        pedir(&app, &Method::GET, "/api/v1/no-existe", Some("Bearer x")).await,
        StatusCode::NOT_FOUND
    );
}

// ------------------------------------------- protección por construcción

async fn sin_extractor() -> &'static str {
    "pasó"
}

/// T5: una ruta declarada `Authenticated` queda protegida **aunque su handler no pida
/// `AuthUser`**. Es la razón de la tabla: olvidarse del extractor ya no abre una puerta.
#[tokio::test]
async fn t5_la_proteccion_no_depende_del_handler() {
    let db = TestDb::migrated().await;
    let state = AppState {
        db: db.conn.clone(),
        pii: pii(),
        jwt: jwt(),
        blobs: jobout_core_api::blob::memory_store(),
    };
    // Una cuenta real, para tener un token válido.
    let (_, cuenta) = {
        let respuesta = app_with_state(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/register")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"email":"t5@correo-prueba.com","password":"una-clave-larga-123",
                               "first_name":"T","last_name":"Cinco"})
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let estado = respuesta.status();
        let bytes = respuesta.into_body().collect().await.unwrap().to_bytes();
        (estado, serde_json::from_slice::<Value>(&bytes).unwrap())
    };
    let token = cuenta["access_token"].as_str().unwrap().to_string();

    let app = table::build(
        vec![
            RouteSpec {
                path: "/prueba/protegida",
                methods: &["GET"],
                access: Access::Authenticated,
                body_limit: None,
                handler: get(sin_extractor),
            },
            RouteSpec {
                path: "/prueba/publica",
                methods: &["GET"],
                access: Access::Public("Ruta de prueba, pública a propósito para este test."),
                body_limit: None,
                handler: get(sin_extractor),
            },
        ],
        state,
    );

    assert_eq!(
        pedir(&app, &Method::GET, "/prueba/protegida", None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        pedir(
            &app,
            &Method::GET,
            "/prueba/protegida",
            Some(&format!("Bearer {token}"))
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        pedir(&app, &Method::GET, "/prueba/publica", None).await,
        StatusCode::OK
    );

    // Y una cuenta borrada pierde el acceso en esa misma ruta.
    db.exec("DELETE FROM users").await.unwrap();
    assert_eq!(
        pedir(
            &app,
            &Method::GET,
            "/prueba/protegida",
            Some(&format!("Bearer {token}"))
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    db.cleanup().await;
}

/// T8: el cuerpo de las peticiones tiene tope en toda la aplicación (8 KB → 413).
#[tokio::test]
async fn t8_el_tope_de_cuerpo_es_global() {
    let app = app_with_state(estado_sin_base());
    let respuesta = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"email":"a@b.cc","password":"x".repeat(9000)}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(respuesta.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

// ------------------------------------------------ límite de cuerpo por ruta

async fn medir(body: axum::body::Bytes) -> String {
    body.len().to_string()
}

async fn subir(app: &Router, ruta: &str, bytes: usize) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(ruta)
                .body(Body::from(vec![b'x'; bytes]))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// T9: una ruta puede declarar su propio tope de cuerpo (la subida de un CV necesita MBs) y eso
/// **no** afecta a las demás, que siguen con el tope global de 8 KB.
#[tokio::test]
async fn t9_el_tope_de_cuerpo_se_puede_declarar_por_ruta() {
    let app = table::build(
        vec![
            RouteSpec {
                path: "/prueba/grande",
                methods: &["POST"],
                access: Access::Public("Ruta de prueba con un tope de cuerpo propio, a propósito."),
                body_limit: Some(64 * 1024),
                handler: axum::routing::post(medir),
            },
            RouteSpec {
                path: "/prueba/normal",
                methods: &["POST"],
                access: Access::Public("Ruta de prueba con el tope global, a propósito."),
                body_limit: None,
                handler: axum::routing::post(medir),
            },
        ],
        estado_sin_base(),
    );

    assert_eq!(
        subir(&app, "/prueba/grande", 20 * 1024).await,
        StatusCode::OK
    );
    assert_eq!(
        subir(&app, "/prueba/grande", 65 * 1024).await,
        StatusCode::PAYLOAD_TOO_LARGE,
        "el tope propio también tiene que cortar"
    );
    assert_eq!(
        subir(&app, "/prueba/normal", 4 * 1024).await,
        StatusCode::OK
    );
    assert_eq!(
        subir(&app, "/prueba/normal", 20 * 1024).await,
        StatusCode::PAYLOAD_TOO_LARGE,
        "las demás rutas conservan el tope global de 8 KB"
    );
}

/// T10: ninguna ruta de la aplicación declara un tope mayor que 6 MB (5 MB de CV más el
/// sobre del multipart). Evita que alguien «arregle» un 413 con un tope enorme.
#[test]
fn t10_ningun_tope_propio_supera_6_mb() {
    for r in table::all() {
        if let Some(n) = r.body_limit {
            assert!(
                n <= 6 * 1024 * 1024,
                "{} declara un tope de {n} bytes",
                r.path
            );
        }
    }
}

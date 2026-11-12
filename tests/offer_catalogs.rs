//! Casos de verdad del endpoint de los catálogos de ofertas (CT4 de docs/working/fase5-ofertas-empresas.md).
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
        blobs: jobout_core_api::blob::memory_store(),
        engine: jobout_core_api::interview::default_engine(),
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

/// Los que el formulario de una oferta lista. `states` y `cities` aceptan un padre opcional.
const NUEVOS: [&str; 8] = [
    "countries",
    "states",
    "cities",
    "salary-ranges",
    "working-shifts",
    "contract-types",
    "employment-sectors",
    "driver-licenses",
];

async fn lista(app: &Router, token: &str, uri: &str) -> Vec<Value> {
    let (e, c) = enviar(app, "GET", uri, Some(token), None).await;
    assert_eq!(e, StatusCode::OK, "{uri}: {c}");
    c.as_array().expect("una lista").clone()
}

fn codigos(filas: &[Value]) -> Vec<&str> {
    filas.iter().map(|f| f["code"].as_str().unwrap()).collect()
}

/// CT4: sin token (o con uno malo) → 401, en todos los catálogos nuevos y también con padre.
#[tokio::test]
async fn ct4_sin_token_es_401() {
    let (db, app, _) = con_cuenta().await;
    let mut uris: Vec<String> = NUEVOS
        .iter()
        .map(|n| format!("/api/v1/catalogs/{n}"))
        .collect();
    uris.push(format!(
        "/api/v1/catalogs/states?country_id={}",
        uuid::Uuid::now_v7()
    ));
    uris.push(format!(
        "/api/v1/catalogs/cities?state_id={}",
        uuid::Uuid::now_v7()
    ));
    for uri in uris {
        let (e, c) = enviar(&app, "GET", &uri, None, None).await;
        assert_eq!(e, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(c["error"], "no_autenticado");
        let (e, _) = enviar(&app, "GET", &uri, Some("esto.no.es-un-jwt"), None).await;
        assert_eq!(e, StatusCode::UNAUTHORIZED, "{uri}");
    }
    db.cleanup().await;
}

/// CT4: mismo formato que los de la Fase 2: solo `id`, `code` y `name`, solo activos, por
/// `sort_order` y luego `name`.
#[tokio::test]
async fn ct4_devuelve_las_activas_ordenadas_con_la_forma_de_siempre() {
    let (db, app, token) = con_cuenta().await;
    db.exec(
        "INSERT INTO working_shifts (id, code, name, active, sort_order) VALUES \
         ('0192b3a0-0000-7000-8000-000000000011', 'zz-inactiva', 'AAA Inactiva', false, -10), \
         ('0192b3a0-0000-7000-8000-000000000012', 'zz-b', 'AAB Empate', true, -5), \
         ('0192b3a0-0000-7000-8000-000000000013', 'zz-a', 'AAA Empate', true, -5), \
         ('0192b3a0-0000-7000-8000-000000000014', 'zz-primera', 'ZZZ Primera', true, -6)",
    )
    .await
    .unwrap();
    let filas = lista(&app, &token, "/api/v1/catalogs/working-shifts").await;
    assert_eq!(&codigos(&filas)[..3], ["zz-primera", "zz-a", "zz-b"]);
    assert!(!codigos(&filas).contains(&"zz-inactiva"));
    let primera = filas[0].as_object().unwrap();
    let mut claves: Vec<&String> = primera.keys().collect();
    claves.sort();
    assert_eq!(claves, ["code", "id", "name"]);
    assert_eq!(primera["id"], "0192b3a0-0000-7000-8000-000000000014");

    // Cada catálogo nuevo trae contenido real y la misma forma exacta.
    for n in NUEVOS {
        let filas = lista(&app, &token, &format!("/api/v1/catalogs/{n}")).await;
        assert!(!filas.is_empty(), "{n} vacío");
        for f in &filas {
            let mut claves: Vec<&String> = f.as_object().unwrap().keys().collect();
            claves.sort();
            assert_eq!(claves, ["code", "id", "name"], "{n}");
        }
    }
    let codigos_de = |n: &str| {
        let n = n.to_string();
        let app = app.clone();
        let token = token.clone();
        async move {
            lista(&app, &token, &format!("/api/v1/catalogs/{n}"))
                .await
                .iter()
                .map(|f| f["code"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(codigos_de("countries").await, ["gt"]);
    assert!(codigos_de("driver-licenses")
        .await
        .contains(&"m".to_string()));
    assert!(codigos_de("contract-types")
        .await
        .contains(&"pasantia".to_string()));
    assert!(codigos_de("employment-sectors")
        .await
        .contains(&"technology".to_string()));
    // Los rangos salariales salen del más bajo al más alto.
    let rangos = codigos_de("salary-ranges").await;
    let esperado: Vec<String> = {
        let mut v = db_orden(&db, "salary_ranges").await;
        v.retain(|c| !c.starts_with("zz-"));
        v
    };
    assert_eq!(rangos, esperado);
    assert!(rangos.len() >= 5);
    assert_eq!(
        rangos.len() as i64,
        db.count("SELECT count(*) FROM salary_ranges WHERE active")
            .await
    );

    // Un catálogo sin activos devuelve una lista vacía, no un error.
    db.exec("UPDATE driver_licenses SET active = false")
        .await
        .unwrap();
    assert_eq!(
        enviar(
            &app,
            "GET",
            "/api/v1/catalogs/driver-licenses",
            Some(&token),
            None
        )
        .await,
        (StatusCode::OK, json!([]))
    );
    db.cleanup().await;
}

/// Códigos de una tabla por `sort_order` y luego `name`, leídos directo de la base.
async fn db_orden(db: &TestDb, tabla: &str) -> Vec<String> {
    use sea_orm::ConnectionTrait;
    db.conn
        .query_all(sea_orm::Statement::from_string(
            db.conn.get_database_backend(),
            format!("SELECT code FROM {tabla} WHERE active ORDER BY sort_order, name"),
        ))
        .await
        .unwrap()
        .iter()
        .map(|r| r.try_get_by_index::<String>(0).unwrap())
        .collect()
}

/// CT4: estados por país y ciudades por estado; sin padre salen todos; con un padre sin hijos,
/// una lista vacía; con un padre que no es un uuid → 400.
#[tokio::test]
async fn ct4_estados_y_ciudades_se_filtran_por_su_padre() {
    let (db, app, token) = con_cuenta().await;
    // Otro país con un estado y una ciudad, para que el filtro tenga algo que excluir.
    let otro = uuid::Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO countries (id, code, name) VALUES ('{otro}', 'zz', 'Zedlandia')"
    ))
    .await
    .unwrap();
    let estado_otro = uuid::Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO states (id, code, name, country_id) \
         VALUES ('{estado_otro}', 'zz-uno', 'Estado Zed', '{otro}')"
    ))
    .await
    .unwrap();
    db.exec(&format!(
        "INSERT INTO cities (id, code, name, state_id) \
         VALUES ('{}', 'zz-uno-ciudad', 'Ciudad Zed', '{estado_otro}')",
        uuid::Uuid::now_v7()
    ))
    .await
    .unwrap();

    let gt = common::uuid_de(&db, "SELECT id FROM countries WHERE code = 'gt'").await;
    let todos = lista(&app, &token, "/api/v1/catalogs/states").await;
    assert_eq!(todos.len(), 23);
    let de_gt = lista(
        &app,
        &token,
        &format!("/api/v1/catalogs/states?country_id={gt}"),
    )
    .await;
    assert_eq!(de_gt.len(), 22);
    assert!(!codigos(&de_gt).contains(&"zz-uno"));
    assert!(codigos(&de_gt).contains(&"guatemala"));
    let nombres: Vec<&str> = de_gt.iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(nombres.first(), Some(&"Alta Verapaz"));
    let de_zz = lista(
        &app,
        &token,
        &format!("/api/v1/catalogs/states?country_id={otro}"),
    )
    .await;
    assert_eq!(codigos(&de_zz), ["zz-uno"]);

    let depto = de_gt.iter().find(|f| f["code"] == "sacatepequez").unwrap();
    let id_depto = depto["id"].as_str().unwrap();
    let ciudades = lista(
        &app,
        &token,
        &format!("/api/v1/catalogs/cities?state_id={id_depto}"),
    )
    .await;
    assert_eq!(ciudades.len(), 16);
    assert!(ciudades.iter().any(|c| c["name"] == "Antigua Guatemala"));
    assert!(ciudades
        .iter()
        .all(|c| c["code"].as_str().unwrap().starts_with("sacatepequez-")));
    assert_eq!(
        lista(&app, &token, "/api/v1/catalogs/cities").await.len(),
        341
    );
    // Padre válido pero sin hijos (o inexistente) → lista vacía.
    for uri in [
        format!("/api/v1/catalogs/cities?state_id={}", uuid::Uuid::now_v7()),
        format!(
            "/api/v1/catalogs/states?country_id={}",
            uuid::Uuid::now_v7()
        ),
    ] {
        assert_eq!(
            enviar(&app, "GET", &uri, Some(&token), None).await,
            (StatusCode::OK, json!([])),
            "{uri}"
        );
    }
    // Un padre que no es uuid → 400, y nada de eso llega al SQL.
    for uri in [
        "/api/v1/catalogs/cities?state_id=no-es-uuid",
        "/api/v1/catalogs/cities?state_id=1%27%20OR%20%271%27%3D%271",
        "/api/v1/catalogs/states?country_id=",
        "/api/v1/catalogs/states?country_id=gt",
    ] {
        let (e, c) = enviar(&app, "GET", uri, Some(&token), None).await;
        assert_eq!(e, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(c["error"], "parametro_invalido", "{uri}");
        assert!(c["message"].as_str().unwrap().len() > 5);
    }
    // El filtro de un catálogo no se acepta en otro: se ignora, no rompe.
    let sin_filtro = lista(
        &app,
        &token,
        "/api/v1/catalogs/working-shifts?country_id=lo-que-sea",
    )
    .await;
    assert!(!sin_filtro.is_empty());
    db.cleanup().await;
}

/// CT4: los catálogos que el formulario no usa siguen sin servirse (404), y los nombres con
/// otra forma tampoco.
#[tokio::test]
async fn ct4_lo_que_no_es_del_formulario_es_404() {
    let (db, app, token) = con_cuenta().await;
    for nombre in [
        "membership-types",
        "zones",
        "salary_ranges",
        "Salary-Ranges",
        "driver_licenses",
        "states%3Bselect",
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
    }
    db.cleanup().await;
}

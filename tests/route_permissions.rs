//! Fase 5, paso 2: control de acceso por permiso en la tabla de rutas (RO1–RO6 de
//! docs/working/fase5-ofertas-empresas.md). Contra PostgreSQL real; una base por prueba.
//!
//! Las rutas de estas pruebas se montan con `table::build` y una lista propia: no hay ninguna
//! ruta de sondeo en el código de producción.

mod common;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use axum::{
    body::Body,
    http::{Request, StatusCode},
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
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{json, Value};
use tower::ServiceExt;

const CLAVE: &str = "una-clave-larga-123";
const RUTA: &str = "/prueba/crear";

fn pii() -> Arc<PiiKeys> {
    Arc::new(
        PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
    )
}

fn jwt() -> Arc<JwtKeys> {
    Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap())
}

fn estado(db: &TestDb) -> AppState {
    AppState {
        db: db.conn.clone(),
        pii: pii(),
        jwt: jwt(),
        blobs: jobout_core_api::blob::memory_store(),
        engine: jobout_core_api::interview::default_engine(),
    }
}

async fn pasa() -> &'static str {
    "pasó"
}

/// Un router con una ruta que exige `offer.create`, otra que exige un permiso que nadie tiene
/// y una solo autenticada.
fn router(state: AppState) -> Router {
    table::build(
        vec![
            RouteSpec {
                path: RUTA,
                methods: &["GET"],
                access: Access::Permission("offer.create"),
                body_limit: None,
                handler: get(pasa),
            },
            RouteSpec {
                path: "/prueba/verificar",
                methods: &["GET"],
                access: Access::Permission("company.verify"),
                body_limit: None,
                handler: get(pasa),
            },
            RouteSpec {
                path: "/prueba/inexistente",
                methods: &["GET"],
                access: Access::Permission("no.existe"),
                body_limit: None,
                handler: get(pasa),
            },
            RouteSpec {
                path: "/prueba/autenticada",
                methods: &["GET"],
                access: Access::Authenticated,
                body_limit: None,
                handler: get(pasa),
            },
        ],
        state,
    )
}

async fn pedir(app: &Router, ruta: &str, token: Option<&str>) -> (StatusCode, String) {
    let mut p = Request::builder().method("GET").uri(ruta);
    if let Some(t) = token {
        p = p.header("authorization", format!("Bearer {t}"));
    }
    let r = app
        .clone()
        .oneshot(p.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let estado = r.status();
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    (estado, String::from_utf8_lossy(&bytes).to_string())
}

async fn registrar(db: &TestDb, email: &str) -> (String, String) {
    let r = app_with_state(estado(db))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"email": email, "password": CLAVE,
                           "first_name": "Ana", "last_name": "Prueba"})
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let token = v["access_token"].as_str().unwrap().to_string();
    let id = jwt().validate(&token).unwrap();
    (token, id)
}

/// Da un rol (por `code`) a una cuenta. Falla con un mensaje claro si el rol no existe.
async fn dar_rol(db: &TestDb, user_id: &str, rol: &str) {
    let r = db
        .conn
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO user_roles (user_id, role_id) \
             SELECT $1::uuid, id FROM roles WHERE code = $2",
            vec![user_id.into(), rol.into()],
        ))
        .await
        .unwrap();
    assert_eq!(r.rows_affected(), 1, "el rol '{rol}' no está sembrado");
}

async fn quitar_rol(db: &TestDb, user_id: &str, rol: &str) {
    db.exec(&format!(
        "DELETE FROM user_roles WHERE user_id = '{user_id}'::uuid \
         AND role_id = (SELECT id FROM roles WHERE code = '{rol}')"
    ))
    .await
    .unwrap();
}

async fn codigos(db: &TestDb, sql: &str) -> Vec<String> {
    db.conn
        .query_all(Statement::from_string(DbBackend::Postgres, sql.to_string()))
        .await
        .unwrap()
        .iter()
        .map(|f| f.try_get_by_index::<String>(0).unwrap())
        .collect()
}

// ------------------------------------------------------------------ la ruta

/// RO1: una cuenta de candidato (sin permiso) recibe 403 `sin_permiso`; el cuerpo es plano
/// `{error, message}` y **no nombra** el permiso exigido ni ninguno otro.
#[tokio::test]
async fn ro1_sin_el_permiso_es_403_y_no_revela_nada() {
    let db = TestDb::migrated().await;
    let app = router(estado(&db));
    let (token, _) = registrar(&db, "cand@correo-prueba.com").await;

    let (st, cuerpo) = pedir(&app, RUTA, Some(&token)).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{cuerpo}");
    let v: Value = serde_json::from_str(&cuerpo).unwrap();
    assert_eq!(v["error"], "sin_permiso");
    assert_eq!(v.as_object().unwrap().len(), 2, "solo error y message");
    assert!(v["message"].as_str().is_some_and(|m| !m.is_empty()));
    for pista in [
        "offer",
        "create",
        "permission",
        "permiso:",
        "company",
        "recruiter",
    ] {
        assert!(
            !cuerpo.to_lowercase().contains(pista),
            "el error filtra '{pista}': {cuerpo}"
        );
    }
    // El mismo cuerpo para un permiso que ni existe: no se distingue.
    let (st2, cuerpo2) = pedir(&app, "/prueba/inexistente", Some(&token)).await;
    assert_eq!(st2, StatusCode::FORBIDDEN);
    assert_eq!(cuerpo, cuerpo2);
    db.cleanup().await;
}

/// Sin token (o con uno malo) sigue siendo 401, no 403.
#[tokio::test]
async fn sin_token_es_401() {
    let db = TestDb::migrated().await;
    let app = router(estado(&db));
    for t in [None, Some("esto.no.es-un-jwt")] {
        let (st, cuerpo) = pedir(&app, RUTA, t).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{cuerpo}");
        assert!(cuerpo.contains("no_autenticado"));
    }
    db.cleanup().await;
}

/// Una cuenta suspendida, aunque tenga el permiso, recibe `cuenta_no_disponible`.
#[tokio::test]
async fn suspendida_con_permiso_es_cuenta_no_disponible() {
    let db = TestDb::migrated().await;
    let app = router(estado(&db));
    let (token, id) = registrar(&db, "susp@correo-prueba.com").await;
    dar_rol(&db, &id, "recruiter").await;
    db.exec(&format!(
        "UPDATE users SET status = 'suspended' WHERE id = '{id}'::uuid"
    ))
    .await
    .unwrap();
    let (st, cuerpo) = pedir(&app, RUTA, Some(&token)).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(cuerpo.contains("cuenta_no_disponible"), "{cuerpo}");
    db.cleanup().await;
}

/// Con el rol, pasa; el permiso viene del rol, y cada rol da solo lo suyo.
#[tokio::test]
async fn el_rol_da_el_permiso_y_solo_ese() {
    let db = TestDb::migrated().await;
    let app = router(estado(&db));
    let (token, id) = registrar(&db, "rec@correo-prueba.com").await;
    dar_rol(&db, &id, "recruiter").await;

    assert_eq!(pedir(&app, RUTA, Some(&token)).await.0, StatusCode::OK);
    assert_eq!(
        pedir(&app, "/prueba/autenticada", Some(&token)).await.0,
        StatusCode::OK
    );
    // company.verify es solo del admin de plataforma: el reclutador no lo tiene.
    assert_eq!(
        pedir(&app, "/prueba/verificar", Some(&token)).await.0,
        StatusCode::FORBIDDEN
    );
    db.cleanup().await;
}

/// RO4: quitar el rol corta el acceso en la **siguiente** petición, con el mismo token; y
/// darlo lo abre igual de rápido. Sin esperar al vencimiento del JWT.
#[tokio::test]
async fn ro4_quitar_el_rol_corta_el_acceso_en_la_siguiente_peticion() {
    let db = TestDb::migrated().await;
    let app = router(estado(&db));
    let (token, id) = registrar(&db, "ro4@correo-prueba.com").await;

    assert_eq!(
        pedir(&app, RUTA, Some(&token)).await.0,
        StatusCode::FORBIDDEN
    );
    dar_rol(&db, &id, "recruiter").await;
    assert_eq!(pedir(&app, RUTA, Some(&token)).await.0, StatusCode::OK);
    assert_eq!(pedir(&app, RUTA, Some(&token)).await.0, StatusCode::OK);
    quitar_rol(&db, &id, "recruiter").await;
    assert_eq!(
        pedir(&app, RUTA, Some(&token)).await.0,
        StatusCode::FORBIDDEN
    );

    // Lo mismo si se quita el permiso al rol, o si el rol se marca como borrado.
    dar_rol(&db, &id, "recruiter").await;
    assert_eq!(pedir(&app, RUTA, Some(&token)).await.0, StatusCode::OK);
    db.exec("UPDATE roles SET deleted_at = now() WHERE code = 'recruiter'")
        .await
        .unwrap();
    assert_eq!(
        pedir(&app, RUTA, Some(&token)).await.0,
        StatusCode::FORBIDDEN
    );
    db.cleanup().await;
}

/// RO5: los permisos salen de `role_permissions`: dar o quitar una fila cambia el acceso sin
/// tocar código.
#[tokio::test]
async fn ro5_los_permisos_salen_de_role_permissions() {
    let db = TestDb::migrated().await;
    let app = router(estado(&db));
    let (token, id) = registrar(&db, "ro5@correo-prueba.com").await;
    dar_rol(&db, &id, "recruiter").await;
    assert_eq!(pedir(&app, RUTA, Some(&token)).await.0, StatusCode::OK);

    db.exec(
        "DELETE FROM role_permissions WHERE role_id = (SELECT id FROM roles WHERE code = 'recruiter') \
         AND permission_id = (SELECT id FROM permissions WHERE code = 'offer.create')",
    )
    .await
    .unwrap();
    assert_eq!(
        pedir(&app, RUTA, Some(&token)).await.0,
        StatusCode::FORBIDDEN
    );

    db.exec(
        "INSERT INTO role_permissions (role_id, permission_id) \
         SELECT r.id, p.id FROM roles r, permissions p \
         WHERE r.code = 'recruiter' AND p.code = 'company.verify'",
    )
    .await
    .unwrap();
    assert_eq!(
        pedir(&app, "/prueba/verificar", Some(&token)).await.0,
        StatusCode::OK
    );
    db.cleanup().await;
}

/// Los permisos de una cuenta no se le pasan a otra.
#[tokio::test]
async fn los_permisos_son_de_cada_cuenta() {
    let db = TestDb::migrated().await;
    let app = router(estado(&db));
    let (tok_a, id_a) = registrar(&db, "a@correo-prueba.com").await;
    let (tok_b, _) = registrar(&db, "b@correo-prueba.com").await;
    dar_rol(&db, &id_a, "company_admin").await;
    assert_eq!(pedir(&app, RUTA, Some(&tok_a)).await.0, StatusCode::OK);
    assert_eq!(
        pedir(&app, RUTA, Some(&tok_b)).await.0,
        StatusCode::FORBIDDEN
    );
    db.cleanup().await;
}

/// La misma cuenta puede ser candidato y reclutador a la vez (decisión 10).
#[tokio::test]
async fn una_cuenta_puede_ser_candidato_y_reclutador() {
    let db = TestDb::migrated().await;
    let app = router(estado(&db));
    let (token, id) = registrar(&db, "doble@correo-prueba.com").await;
    dar_rol(&db, &id, "recruiter").await;
    let roles = codigos(
        &db,
        &format!(
            "SELECT r.code FROM user_roles ur JOIN roles r ON r.id = ur.role_id \
             WHERE ur.user_id = '{id}'::uuid ORDER BY 1"
        ),
    )
    .await;
    assert_eq!(roles, ["applicant", "recruiter"]);
    assert_eq!(pedir(&app, RUTA, Some(&token)).await.0, StatusCode::OK);
    db.cleanup().await;
}

/// Una petición protegida hace **una** consulta a la base (la de la cuenta, que ya trae los
/// permisos), no una por permiso ni una por rol.
#[tokio::test]
async fn los_permisos_viajan_en_la_consulta_de_la_cuenta() {
    let db = TestDb::migrated().await;
    let (token, id) = registrar(&db, "una@correo-prueba.com").await;
    dar_rol(&db, &id, "company_admin").await;

    let cuenta = Arc::new(AtomicUsize::new(0));
    let mut conn = db.conn.clone();
    let c = cuenta.clone();
    conn.set_metric_callback(move |_| {
        c.fetch_add(1, Ordering::SeqCst);
    });
    let app = router(AppState {
        db: conn,
        ..estado(&db)
    });
    assert_eq!(pedir(&app, RUTA, Some(&token)).await.0, StatusCode::OK);
    assert_eq!(cuenta.load(Ordering::SeqCst), 1);
    assert_eq!(pedir(&app, RUTA, Some(&token)).await.0, StatusCode::OK);
    assert_eq!(cuenta.load(Ordering::SeqCst), 2);
    db.cleanup().await;
}

// ------------------------------------------------------------------- semillas

/// La asignación exacta de permisos por rol. Es la política: cambiarla obliga a editar esto.
#[tokio::test]
async fn los_permisos_sembrados_por_rol_son_exactamente_estos() {
    let db = TestDb::migrated().await;
    let por_rol = |rol: &'static str| {
        format!(
            "SELECT p.code FROM role_permissions rp \
             JOIN roles r ON r.id = rp.role_id JOIN permissions p ON p.id = rp.permission_id \
             WHERE r.code = '{rol}' ORDER BY 1"
        )
    };
    let recruiter = [
        "application.read",
        "application.review",
        "favorite.manage",
        "offer.close",
        "offer.create",
        "offer.publish",
        "offer.update",
    ];
    assert_eq!(codigos(&db, &por_rol("recruiter")).await, recruiter);
    let mut company_admin: Vec<&str> = recruiter.to_vec();
    company_admin.extend(["company.invite", "company.update"]);
    company_admin.sort();
    assert_eq!(codigos(&db, &por_rol("company_admin")).await, company_admin);
    assert_eq!(codigos(&db, &por_rol("admin")).await, ["company.verify"]);
    assert!(codigos(&db, &por_rol("applicant")).await.is_empty());

    // Los roles de la fase 1 siguen ahí, y los nuevos tienen nombre.
    let roles = codigos(&db, "SELECT code FROM roles ORDER BY 1").await;
    assert_eq!(roles, ["admin", "applicant", "company_admin", "recruiter"]);
    let permisos = codigos(&db, "SELECT code FROM permissions ORDER BY 1").await;
    assert_eq!(permisos.len(), 10);
    db.cleanup().await;
}

/// La migración se deshace sin tocar los roles de la fase 1 y se puede volver a aplicar.
#[tokio::test]
async fn la_migracion_baja_y_sube() {
    use migration::{Migrator, MigratorTrait};
    let db = TestDb::migrated().await;
    let (_, id) = registrar(&db, "down@correo-prueba.com").await;
    dar_rol(&db, &id, "recruiter").await;
    Migrator::down(&db.conn, Some(7)).await.expect("down");
    assert_eq!(
        codigos(&db, "SELECT code FROM roles ORDER BY 1").await,
        ["admin", "applicant"]
    );
    assert_eq!(db.count("SELECT count(*) FROM permissions").await, 0);
    assert_eq!(db.count("SELECT count(*) FROM user_roles").await, 1);
    Migrator::up(&db.conn, None).await.expect("up otra vez");
    assert_eq!(db.count("SELECT count(*) FROM roles").await, 4);
    db.cleanup().await;
}

/// RO3 (parte de datos): todo permiso que una ruta real exige existe sembrado, así una ruta no
/// puede quedar cerrada para todos por un error de tipeo.
#[tokio::test]
async fn todo_permiso_exigido_por_la_tabla_existe_en_la_base() {
    let db = TestDb::migrated().await;
    let sembrados = codigos(&db, "SELECT code FROM permissions").await;
    for r in table::all() {
        if let Access::Permission(p) = r.access {
            assert!(sembrados.iter().any(|s| s == p), "{} exige '{p}'", r.path);
        }
    }
    db.cleanup().await;
}

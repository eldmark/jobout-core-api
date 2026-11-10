//! Casos de verdad de las ofertas del reclutador (OF1–OF12, RO, SX del paso 4 de
//! docs/working/fase5-ofertas-empresas.md), contra el contrato «Ofertas del reclutador» de
//! `api.md`. PostgreSQL real, una base por prueba. Se escribieron antes que el código, en rojo.
//!
//! Las empresas, membresías y reclutadores se insertan por SQL (`empresa`, `miembro`): estas
//! pruebas no dependen de las rutas de empresas. Ningún reloj de pared: las fechas salen de
//! `current_date` de la base más un desplazamiento, y el cierre automático recibe «ahora».

#[allow(dead_code)]
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
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{json, Value};
use tower::ServiceExt;

const CLAVE: &str = "una-clave-larga-123";
const OFERTAS: &str = "/api/v1/company/offers";

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

fn app(db: &TestDb) -> Router {
    app_with_state(estado(db))
}

async fn enviar_crudo(
    app: &Router,
    metodo: &str,
    uri: &str,
    token: Option<&str>,
    cuerpo: Body,
) -> (StatusCode, Value, axum::http::HeaderMap) {
    let mut p = Request::builder()
        .method(metodo)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = token {
        p = p.header("authorization", format!("Bearer {t}"));
    }
    let r = app.clone().oneshot(p.body(cuerpo).unwrap()).await.unwrap();
    let estado = r.status();
    let cabeceras = r.headers().clone();
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    (
        estado,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        cabeceras,
    )
}

async fn enviar(
    app: &Router,
    metodo: &str,
    uri: &str,
    token: Option<&str>,
    cuerpo: Option<Value>,
) -> (StatusCode, Value) {
    let cuerpo = cuerpo.map_or(Body::empty(), |j| Body::from(j.to_string()));
    let (e, v, _) = enviar_crudo(app, metodo, uri, token, cuerpo).await;
    (e, v)
}

// ------------------------------------------------------------------ fixtures

/// Registra una cuenta y devuelve (token, user_id).
async fn registrar(app: &Router, email: &str) -> (String, String) {
    let (e, v) = enviar(
        app,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(json!({"email": email, "password": CLAVE, "first_name": "Ana", "last_name": "Paz"})),
    )
    .await;
    assert_eq!(e, StatusCode::CREATED, "{v}");
    let token = v["access_token"].as_str().unwrap().to_string();
    let id = jwt().validate(&token).unwrap();
    (token, id)
}

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

async fn texto(db: &TestDb, sql: &str) -> String {
    db.conn
        .query_one(Statement::from_string(DbBackend::Postgres, sql.to_string()))
        .await
        .unwrap()
        .expect("una fila")
        .try_get_by_index::<String>(0)
        .unwrap()
}

/// Vigencia de la membresía de una empresa de prueba.
#[derive(Clone, Copy, PartialEq)]
enum Membresia {
    Vigente,
    Vencida,
    Ninguna,
}

/// Una empresa de prueba por SQL. El NIT se guarda como bytes opacos (aquí no se descifra).
async fn empresa(db: &TestDb, nombre: &str, verificada: bool, m: Membresia) -> String {
    let id = common::uuid_de(db, "SELECT gen_random_uuid()")
        .await
        .to_string();
    let pais = texto(db, "SELECT id::text FROM countries ORDER BY code LIMIT 1").await;
    let sector = texto(
        db,
        "SELECT id::text FROM employment_sectors ORDER BY code LIMIT 1",
    )
    .await;
    db.exec(&format!(
        "INSERT INTO companies (id, name, tax_id, tax_id_hash, country_id, employment_sector_id, verified_at) \
         VALUES ('{id}', '{nombre}', '\\x01'::bytea, '\\x{}'::bytea, '{pais}', '{sector}', {})",
        common::hex(nombre.as_bytes()),
        if verificada { "now()" } else { "NULL" }
    ))
    .await
    .unwrap();
    let tipo = texto(
        db,
        "SELECT id::text FROM membership_types ORDER BY code LIMIT 1",
    )
    .await;
    let (ini, fin) = match m {
        Membresia::Vigente => ("current_date - 10", "current_date + 100"),
        Membresia::Vencida => ("current_date - 100", "current_date - 1"),
        Membresia::Ninguna => return id,
    };
    db.exec(&format!(
        "INSERT INTO memberships (id, company_id, membership_type_id, start_date, expiration_date, status) \
         VALUES (gen_random_uuid(), '{id}', '{tipo}', {ini}, {fin}, 'active')"
    ))
    .await
    .unwrap();
    id
}

/// Hace de la cuenta miembro de la empresa con ese rol. Devuelve el id de `company_recruiters`.
async fn miembro(db: &TestDb, user_id: &str, company_id: &str, rol: &str) -> String {
    dar_rol(db, user_id, rol).await;
    let id = common::uuid_de(db, "SELECT gen_random_uuid()")
        .await
        .to_string();
    db.exec(&format!(
        "INSERT INTO company_recruiters (id, user_id, company_id, role, position) \
         VALUES ('{id}', '{user_id}', '{company_id}', '{}', 'Reclutadora')",
        if rol == "company_admin" {
            "company_admin"
        } else {
            "recruiter"
        }
    ))
    .await
    .unwrap();
    id
}

/// Un reclutador completo: cuenta + rol + membresía en la empresa. Devuelve su token.
async fn reclutador(db: &TestDb, app: &Router, email: &str, company_id: &str, rol: &str) -> String {
    let (token, uid) = registrar(app, email).await;
    miembro(db, &uid, company_id, rol).await;
    token
}

/// Un mundo con dos empresas verificadas y vigentes, cada una con su admin.
struct Mundo {
    db: TestDb,
    app: Router,
    cat: Cat,
    emp_a: String,
    emp_b: String,
    a: String,
    b: String,
}

async fn mundo() -> Mundo {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let emp_a = empresa(&db, "Empresa A", true, Membresia::Vigente).await;
    let emp_b = empresa(&db, "Empresa B", true, Membresia::Vigente).await;
    let a = reclutador(&db, &app, "a@correo-prueba.com", &emp_a, "company_admin").await;
    let b = reclutador(&db, &app, "b@correo-prueba.com", &emp_b, "company_admin").await;
    // El catálogo sembrado trae pocas licencias e idiomas: se completan para probar los topes.
    for i in 0..6 {
        db.exec(&format!(
            "INSERT INTO languages (id, code, name) VALUES (gen_random_uuid(), 'prueba-{i}', 'Prueba {i}')"
        ))
        .await
        .unwrap();
        db.exec(&format!(
            "INSERT INTO driver_licenses (id, code, name) VALUES (gen_random_uuid(), 'prueba-{i}', 'Prueba {i}')"
        ))
        .await
        .unwrap();
    }
    let cat = Cat::cargar(&db).await;
    Mundo {
        db,
        app,
        cat,
        emp_a,
        emp_b,
        a,
        b,
    }
}

/// Ids de los catálogos que usa una oferta.
struct Cat {
    profession: String,
    sector: String,
    hierarchy: String,
    level: String,
    state: String,
    city: String,
    otra_ciudad_de_otro_estado: String,
    salary: String,
    shift: String,
    contract: String,
    languages: Vec<String>,
    skills: Vec<String>,
    licenses: Vec<String>,
}

impl Cat {
    async fn cargar(db: &TestDb) -> Self {
        async fn ids(db: &TestDb, sql: &str) -> Vec<String> {
            db.conn
                .query_all(Statement::from_string(DbBackend::Postgres, sql.to_string()))
                .await
                .unwrap()
                .iter()
                .map(|f| f.try_get_by_index::<String>(0).unwrap())
                .collect()
        }
        let uno = |t: &str| format!("SELECT id::text FROM {t} WHERE active ORDER BY code LIMIT 1");
        let state = texto(
            db,
            "SELECT s.id::text FROM states s WHERE s.code = 'guatemala'",
        )
        .await;
        let city = texto(
            db,
            &format!(
                "SELECT id::text FROM cities WHERE state_id = '{state}' ORDER BY code LIMIT 1"
            ),
        )
        .await;
        let otra = texto(
            db,
            &format!(
                "SELECT id::text FROM cities WHERE state_id <> '{state}' ORDER BY code LIMIT 1"
            ),
        )
        .await;
        Self {
            profession: texto(db, &uno("professions")).await,
            sector: texto(db, &uno("employment_sectors")).await,
            hierarchy: texto(db, &uno("job_hierarchies")).await,
            level: texto(db, &uno("academic_levels")).await,
            state,
            city,
            otra_ciudad_de_otro_estado: otra,
            salary: texto(db, &uno("salary_ranges")).await,
            shift: texto(db, &uno("working_shifts")).await,
            contract: texto(db, &uno("contract_types")).await,
            languages: ids(db, "SELECT id::text FROM languages ORDER BY code LIMIT 8").await,
            skills: ids(db, "SELECT id::text FROM skills ORDER BY code LIMIT 40").await,
            licenses: ids(
                db,
                "SELECT id::text FROM driver_licenses ORDER BY code LIMIT 8",
            )
            .await,
        }
    }

    /// Una oferta mínima publicable (con una skill de requisito).
    fn completa(&self) -> Value {
        json!({
            "title": "Desarrollador backend",
            "description": "Construirás APIs en Rust para el equipo de producto.",
            "vacancies": 2,
            "profession_id": self.profession,
            "state_id": self.state,
            "city_id": self.city,
            "skills": [{"skill_id": self.skills[0], "is_required": true, "min_level": "intermediate"}],
        })
    }

    /// El cuerpo más completo posible: todos los campos del contrato.
    fn total(&self, hiring: &str) -> Value {
        json!({
            "title": "Desarrollador backend", "description": "Construirás APIs en Rust para el equipo.",
            "is_confidential": false, "is_experience_required": true, "required_experience": 2,
            "main_language_id": self.languages[0], "schedule": "Lunes a viernes, 8 a 17",
            "required_availability_to_travel": false, "vacancies": 2, "hiring_date": hiring,
            "profession_id": self.profession, "employment_sector_id": self.sector,
            "job_hierarchy_id": self.hierarchy, "minimum_academic_level_id": self.level,
            "state_id": self.state, "city_id": self.city, "salary_range_id": self.salary,
            "working_shift_id": self.shift, "contract_type_id": self.contract,
            "skills": [{"skill_id": self.skills[0], "is_required": true, "min_level": "intermediate"},
                       {"skill_id": self.skills[1], "is_required": false, "min_level": "beginner"}],
            "languages": [{"language_id": self.languages[1], "min_proficiency": "intermediate"}],
            "driver_license_ids": [self.licenses[0]],
        })
    }
}

/// Una fecha `YYYY-MM-DD` a `dias` de hoy (UTC), según la base.
async fn fecha(db: &TestDb, dias: i32) -> String {
    texto(
        db,
        &format!("SELECT ((now() AT TIME ZONE 'UTC')::date + {dias})::text"),
    )
    .await
}

async fn crear(m: &Mundo, token: &str, cuerpo: Value) -> (StatusCode, Value) {
    enviar(&m.app, "POST", OFERTAS, Some(token), Some(cuerpo)).await
}

/// Crea una oferta y devuelve su id (falla si no da 201).
async fn crear_ok(m: &Mundo, token: &str, cuerpo: Value) -> String {
    let (e, v) = crear(m, token, cuerpo).await;
    assert_eq!(e, StatusCode::CREATED, "{v}");
    v["id"].as_str().unwrap().to_string()
}

/// Crea una completa y la publica.
async fn publicada(m: &Mundo, token: &str) -> String {
    let id = crear_ok(m, token, m.cat.completa()).await;
    let (e, v) = accion(m, token, &id, "publish").await;
    assert_eq!(e, StatusCode::OK, "{v}");
    id
}

async fn accion(m: &Mundo, token: &str, id: &str, que: &str) -> (StatusCode, Value) {
    enviar(
        &m.app,
        "POST",
        &format!("{OFERTAS}/{id}/{que}"),
        Some(token),
        None,
    )
    .await
}

async fn leer(m: &Mundo, token: &str, id: &str) -> Value {
    let (e, v) = enviar(&m.app, "GET", &format!("{OFERTAS}/{id}"), Some(token), None).await;
    assert_eq!(e, StatusCode::OK, "{v}");
    v
}

async fn put(m: &Mundo, token: &str, id: &str, cuerpo: Value) -> (StatusCode, Value) {
    enviar(
        &m.app,
        "PUT",
        &format!("{OFERTAS}/{id}"),
        Some(token),
        Some(cuerpo),
    )
    .await
}

async fn estado_de(m: &Mundo, id: &str) -> String {
    texto(
        &m.db,
        &format!("SELECT status::text FROM job_offers WHERE id = '{id}'"),
    )
    .await
}

fn codigo(v: &Value) -> &str {
    v["error"].as_str().unwrap_or("")
}

// ============================================================ OF1 – OF3

/// OF1: crear una oferta con título, descripción y requisitos → 201 en `draft`, con la forma
/// del contrato; la lee su empresa.
#[tokio::test]
async fn of1_crear_una_oferta_en_draft() {
    let m = mundo().await;
    let hiring = fecha(&m.db, 60).await;
    let (e, v) = crear(&m, &m.a, m.cat.total(&hiring)).await;
    assert_eq!(e, StatusCode::CREATED, "{v}");
    assert_eq!(v["status"], "draft");
    assert_eq!(v["publication_date"], Value::Null);
    assert_eq!(v["closed_at"], Value::Null);
    assert_eq!(v["applications_count"], 0);
    assert_eq!(v["required_driver_license"], true);
    assert_eq!(v["title"], "Desarrollador backend");
    assert_eq!(v["hiring_date"], hiring.as_str());
    assert_eq!(v["skills"].as_array().unwrap().len(), 2);
    assert_eq!(v["languages"][0]["min_proficiency"], "intermediate");
    assert_eq!(v["driver_license_ids"], json!([m.cat.licenses[0]]));
    assert_eq!(v["created_by"]["display_name"], "Ana P.");
    assert!(v["created_by"]["id"].is_string());
    assert!(v["created_at"].as_str().unwrap().ends_with('Z'));
    // company_visible no sale de la API; tampoco featured.
    for prohibido in ["company_visible", "featured", "company_id", "name"] {
        assert!(
            v.get(prohibido).is_none(),
            "la API no debe exponer {prohibido}"
        );
    }
    let id = v["id"].as_str().unwrap();
    assert_eq!(leer(&m, &m.a, id).await, v);
    assert_eq!(
        m.db.count("SELECT count(*) FROM job_offers WHERE status = 'draft'")
            .await,
        1
    );
    m.db.cleanup().await;
}

/// Solo `title` es obligatorio al crear: un borrador casi vacío es válido.
#[tokio::test]
async fn solo_el_titulo_es_obligatorio_al_crear() {
    let m = mundo().await;
    let (e, v) = crear(&m, &m.a, json!({"title": "Borrador"})).await;
    assert_eq!(e, StatusCode::CREATED, "{v}");
    assert_eq!(v["description"], Value::Null);
    assert_eq!(v["skills"], json!([]));
    assert_eq!(v["required_driver_license"], false);
    assert_eq!(v["is_confidential"], false);
    let (e, v) = crear(&m, &m.a, json!({})).await;
    assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(codigo(&v), "datos_invalidos");
    m.db.cleanup().await;
}

/// OF2: `description` existe una sola vez, en la tabla y en la respuesta.
#[tokio::test]
async fn of2_la_descripcion_existe_una_sola_vez() {
    let m = mundo().await;
    let n =
        m.db.count(
            "SELECT count(*) FROM information_schema.columns \
             WHERE table_schema = 'public' AND table_name = 'job_offers' \
             AND column_name LIKE '%description%'",
        )
        .await;
    assert_eq!(n, 1);
    let id = crear_ok(&m, &m.a, m.cat.completa()).await;
    let v = leer(&m, &m.a, &id).await;
    let claves: Vec<&String> = v
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| k.contains("descr"))
        .collect();
    assert_eq!(claves.len(), 1);
    m.db.cleanup().await;
}

/// OF3: los requisitos van en las tres tablas, y `PUT` los reemplaza.
#[tokio::test]
async fn of3_los_requisitos_van_en_tablas_propias() {
    let m = mundo().await;
    let hiring = fecha(&m.db, 60).await;
    let id = crear_ok(&m, &m.a, m.cat.total(&hiring)).await;
    let c = |t: &str| format!("SELECT count(*) FROM {t} WHERE job_offer_id = '{id}'");
    assert_eq!(m.db.count(&c("job_offer_skills")).await, 2);
    assert_eq!(m.db.count(&c("job_offer_languages")).await, 1);
    assert_eq!(m.db.count(&c("job_offer_driver_licenses")).await, 1);
    let (e, v) = put(&m, &m.a, &id, m.cat.completa()).await;
    assert_eq!(e, StatusCode::NO_CONTENT, "{v}");
    assert_eq!(m.db.count(&c("job_offer_skills")).await, 1);
    assert_eq!(m.db.count(&c("job_offer_languages")).await, 0);
    assert_eq!(m.db.count(&c("job_offer_driver_licenses")).await, 0);
    assert_eq!(leer(&m, &m.a, &id).await["required_driver_license"], false);
    m.db.cleanup().await;
}

// ================================================================== OF4

/// OF4: ningún campo de edad, género ni estado civil (ni los de plataforma) entra: 422
/// `campo_no_permitido` y nada se guarda.
#[tokio::test]
async fn of4_campos_discriminatorios_y_de_plataforma_dan_422() {
    let m = mundo().await;
    for campo in [
        "age_range_from",
        "age_range_to",
        "age",
        "gender",
        "gender_preference_id",
        "marital_status_id",
        "marital_status",
        "featured",
        "status",
        "id",
        "company_id",
        "created_by",
        "publication_date",
        "required_driver_license",
        "company_visible",
        "match_score",
        "sort",
    ] {
        let mut cuerpo = m.cat.completa();
        cuerpo[campo] = json!("30");
        let (e, v) = crear(&m, &m.a, cuerpo.clone()).await;
        assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{campo}: {v}");
        assert_eq!(codigo(&v), "campo_no_permitido", "{campo}");
        assert!(
            !v["message"].to_string().contains(campo),
            "el error no repite el campo"
        );
        // también en el PUT
        let id = crear_ok(&m, &m.a, m.cat.completa()).await;
        let (e, v) = put(&m, &m.a, &id, cuerpo).await;
        assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{campo}");
        assert_eq!(codigo(&v), "campo_no_permitido");
        m.db.exec("DELETE FROM job_offers").await.unwrap();
    }
    // Campos desconocidos dentro de los requisitos.
    let mut cuerpo = m.cat.completa();
    cuerpo["skills"][0]["gender"] = json!("f");
    let (e, v) = crear(&m, &m.a, cuerpo).await;
    assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codigo(&v), "campo_no_permitido");
    assert_eq!(m.db.count("SELECT count(*) FROM job_offers").await, 0);
    m.db.cleanup().await;
}

/// OF4: en la consulta, un parámetro desconocido (edad, género, `sort=match`) es 422
/// `parametro_no_permitido`.
#[tokio::test]
async fn of4_parametros_desconocidos_en_la_consulta_dan_422() {
    let m = mundo().await;
    for q in [
        "age=30",
        "gender=f",
        "marital_status=soltero",
        "sort=match",
        "min_age=18",
        "q=rust",
    ] {
        let (e, v) = enviar(&m.app, "GET", &format!("{OFERTAS}?{q}"), Some(&m.a), None).await;
        assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{q}: {v}");
        assert_eq!(codigo(&v), "parametro_no_permitido", "{q}");
    }
    m.db.cleanup().await;
}

// ================================================================== OF5

async fn faltantes(m: &Mundo, token: &str, id: &str) -> Vec<String> {
    let (e, v) = accion(m, token, id, "publish").await;
    assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(codigo(&v), "oferta_incompleta");
    assert_eq!(v["message"], "Faltan datos para publicar la oferta.");
    v["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

/// OF5: publicar valida lo mínimo y `fields` lista lo que falta.
#[tokio::test]
async fn of5_publicar_valida_lo_minimo() {
    let m = mundo().await;
    let id = crear_ok(&m, &m.a, json!({"title": "Solo título"})).await;
    assert_eq!(
        faltantes(&m, &m.a, &id).await,
        [
            "description",
            "vacancies",
            "profession_id",
            "state_id",
            "city_id",
            "requirements"
        ]
    );
    assert_eq!(estado_de(&m, &id).await, "draft");

    // Cada campo, por separado.
    let base = m.cat.completa();
    for (campo, esperado) in [
        ("description", "description"),
        ("vacancies", "vacancies"),
        ("profession_id", "profession_id"),
        ("city_id", "city_id"),
        ("skills", "requirements"),
    ] {
        let mut cuerpo = base.clone();
        cuerpo.as_object_mut().unwrap().remove(campo);
        let id = crear_ok(&m, &m.a, cuerpo).await;
        assert_eq!(faltantes(&m, &m.a, &id).await, [esperado], "{campo}");
    }
    // Sin estado, tampoco hay ciudad válida: faltan los dos.
    let mut cuerpo = base.clone();
    cuerpo.as_object_mut().unwrap().remove("state_id");
    cuerpo.as_object_mut().unwrap().remove("city_id");
    let id = crear_ok(&m, &m.a, cuerpo).await;
    assert_eq!(faltantes(&m, &m.a, &id).await, ["state_id", "city_id"]);

    // Un solo requisito basta: skill, idioma o licencia.
    for requisito in [
        json!({"languages": [{"language_id": m.cat.languages[0], "min_proficiency": "basic"}]}),
        json!({"driver_license_ids": [m.cat.licenses[0]]}),
        json!({"skills": [{"skill_id": m.cat.skills[0], "is_required": false, "min_level": null}]}),
    ] {
        let mut cuerpo = m.cat.completa();
        cuerpo.as_object_mut().unwrap().remove("skills");
        for (k, val) in requisito.as_object().unwrap() {
            cuerpo[k] = val.clone();
        }
        let id = crear_ok(&m, &m.a, cuerpo).await;
        let (e, v) = accion(&m, &m.a, &id, "publish").await;
        assert_eq!(e, StatusCode::OK, "{v}");
        assert_eq!(v["status"], "published");
    }
    m.db.cleanup().await;
}

/// `hiring_date` pasada también es `oferta_incompleta` (se vence entre el borrador y publicar).
#[tokio::test]
async fn una_fecha_de_contratacion_pasada_no_deja_publicar() {
    let m = mundo().await;
    let hiring = fecha(&m.db, 30).await;
    let mut cuerpo = m.cat.completa();
    cuerpo["hiring_date"] = json!(hiring);
    let id = crear_ok(&m, &m.a, cuerpo).await;
    m.db.exec(&format!(
        "UPDATE job_offers SET hiring_date = current_date - 1 WHERE id = '{id}'"
    ))
    .await
    .unwrap();
    assert_eq!(faltantes(&m, &m.a, &id).await, ["hiring_date"]);
    m.db.cleanup().await;
}

// ================================================================== OF6

/// OF6: `draft → published → paused → published → closed`, y `publication_date` solo en la
/// primera publicación.
#[tokio::test]
async fn of6_transiciones_validas() {
    let m = mundo().await;
    let id = crear_ok(&m, &m.a, m.cat.completa()).await;
    let (e, v) = accion(&m, &m.a, &id, "publish").await;
    assert_eq!(e, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "published");
    assert_eq!(v["id"], id.as_str());
    let hoy = fecha(&m.db, 0).await;
    assert_eq!(v["publication_date"], hoy.as_str());

    // La primera fecha se conserva aunque pasen los días.
    m.db.exec(&format!(
        "UPDATE job_offers SET publication_date = current_date - 5 WHERE id = '{id}'"
    ))
    .await
    .unwrap();
    let antigua = fecha(&m.db, -5).await;

    let (e, _) = accion(&m, &m.a, &id, "pause").await;
    assert_eq!(e, StatusCode::NO_CONTENT);
    assert_eq!(estado_de(&m, &id).await, "paused");
    let (e, v) = accion(&m, &m.a, &id, "publish").await;
    assert_eq!(e, StatusCode::OK, "{v}");
    assert_eq!(v["publication_date"], antigua.as_str());
    let (e, _) = accion(&m, &m.a, &id, "close").await;
    assert_eq!(e, StatusCode::NO_CONTENT);
    assert_eq!(estado_de(&m, &id).await, "closed");
    let cerrada = leer(&m, &m.a, &id).await;
    assert_eq!(cerrada["status"], "closed");
    assert!(cerrada["closed_at"].is_string());
    assert_eq!(cerrada["publication_date"], antigua.as_str());
    m.db.cleanup().await;
}

/// OF6: desde `paused` también se cierra.
#[tokio::test]
async fn una_pausada_se_puede_cerrar() {
    let m = mundo().await;
    let id = publicada(&m, &m.a).await;
    accion(&m, &m.a, &id, "pause").await;
    let (e, _) = accion(&m, &m.a, &id, "close").await;
    assert_eq!(e, StatusCode::NO_CONTENT);
    assert!(leer(&m, &m.a, &id).await["closed_at"].is_string());
    m.db.cleanup().await;
}

/// OF6: toda transición inválida es 409 `estado_invalido` y no toca nada; `closed` es final.
#[tokio::test]
async fn of6_transiciones_invalidas_son_409_y_no_cambian_nada() {
    let m = mundo().await;
    // (estado de partida, acción) que NO se permiten.
    let invalidas = [
        ("draft", "pause"),
        ("draft", "close"),
        ("published", "publish"),
        ("paused", "pause"),
        ("closed", "publish"),
        ("closed", "pause"),
        ("closed", "close"),
    ];
    for (inicio, que) in invalidas {
        let id = crear_ok(&m, &m.a, m.cat.completa()).await;
        match inicio {
            "draft" => {}
            "published" => {
                accion(&m, &m.a, &id, "publish").await;
            }
            "paused" => {
                accion(&m, &m.a, &id, "publish").await;
                accion(&m, &m.a, &id, "pause").await;
            }
            "closed" => {
                accion(&m, &m.a, &id, "publish").await;
                accion(&m, &m.a, &id, "close").await;
            }
            _ => unreachable!(),
        }
        let antes = texto(
            &m.db,
            &format!("SELECT row(o.*)::text FROM job_offers o WHERE id = '{id}'"),
        )
        .await;
        let (e, v) = accion(&m, &m.a, &id, que).await;
        assert_eq!(e, StatusCode::CONFLICT, "{inicio} + {que}: {v}");
        assert_eq!(codigo(&v), "estado_invalido", "{inicio} + {que}");
        let despues = texto(
            &m.db,
            &format!("SELECT row(o.*)::text FROM job_offers o WHERE id = '{id}'"),
        )
        .await;
        assert_eq!(antes, despues, "{inicio} + {que} tocó la fila");
    }
    m.db.cleanup().await;
}

/// Una cerrada no se edita (409) ni se borra (409).
#[tokio::test]
async fn una_cerrada_no_se_edita_ni_se_borra() {
    let m = mundo().await;
    let id = publicada(&m, &m.a).await;
    accion(&m, &m.a, &id, "close").await;
    let (e, v) = put(&m, &m.a, &id, m.cat.completa()).await;
    assert_eq!(e, StatusCode::CONFLICT);
    assert_eq!(codigo(&v), "estado_invalido");
    let (e, v) = enviar(
        &m.app,
        "DELETE",
        &format!("{OFERTAS}/{id}"),
        Some(&m.a),
        None,
    )
    .await;
    assert_eq!(e, StatusCode::CONFLICT);
    assert_eq!(codigo(&v), "estado_invalido");
    m.db.cleanup().await;
}

/// Concurrencia: dos `publish` simultáneos dejan una sola publicación; la otra recibe 409.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dos_publicaciones_simultaneas_dejan_una() {
    let m = mundo().await;
    for _ in 0..5 {
        let id = crear_ok(&m, &m.a, m.cat.completa()).await;
        let (r1, r2) = tokio::join!(
            accion(&m, &m.a, &id, "publish"),
            accion(&m, &m.a, &id, "publish")
        );
        let mut estados = [r1.0.as_u16(), r2.0.as_u16()];
        estados.sort();
        assert_eq!(estados, [200, 409], "{r1:?} {r2:?}");
        assert_eq!(estado_de(&m, &id).await, "published");
        assert_eq!(
            m.db.count(&format!(
                "SELECT count(*) FROM job_offers WHERE id = '{id}' AND publication_date IS NOT NULL"
            ))
            .await,
            1
        );
    }
    m.db.cleanup().await;
}

/// Concurrencia: reanudar y cerrar a la vez termina en un estado coherente (`closed` si el
/// cierre ganó, y nunca un estado distinto de `closed` con `closed_at`, ni al revés).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publicar_y_cerrar_a_la_vez_es_consistente() {
    let m = mundo().await;
    for _ in 0..5 {
        let id = publicada(&m, &m.a).await;
        accion(&m, &m.a, &id, "pause").await;
        let (r1, r2) = tokio::join!(
            accion(&m, &m.a, &id, "publish"),
            accion(&m, &m.a, &id, "close")
        );
        let final_ = estado_de(&m, &id).await;
        assert!(r1.0 == 200 || r1.0 == 409, "{r1:?}");
        assert!(r2.0 == 204 || r2.0 == 409, "{r2:?}");
        // Exactamente una de las dos ganó, o el cierre ganó tras la reanudación.
        assert!(final_ == "closed" || final_ == "published");
        assert_eq!(
            m.db.count(&format!(
                "SELECT count(*) FROM job_offers WHERE id = '{id}' \
                 AND ((status = 'closed') <> (closed_at IS NOT NULL))"
            ))
            .await,
            0,
            "estado final {final_}"
        );
    }
    m.db.cleanup().await;
}

/// Dos `pause` simultáneos: uno 204, otro 409.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dos_pausas_simultaneas_dejan_una() {
    let m = mundo().await;
    let id = publicada(&m, &m.a).await;
    let (r1, r2) = tokio::join!(
        accion(&m, &m.a, &id, "pause"),
        accion(&m, &m.a, &id, "pause")
    );
    let mut estados = [r1.0.as_u16(), r2.0.as_u16()];
    estados.sort();
    assert_eq!(estados, [204, 409]);
    m.db.cleanup().await;
}

// ================================================================== OF7

/// OF7: desde la primera aplicación solo se editan `description`, `schedule`, `vacancies` y
/// `hiring_date`; lo demás → 409 `oferta_con_aplicaciones`. Las aplicaciones todavía no existen
/// (paso 6): se simulan con el contador transitorio `applications_count`.
#[tokio::test]
async fn of7_con_aplicaciones_solo_se_editan_cuatro_campos() {
    let m = mundo().await;
    let hiring = fecha(&m.db, 60).await;
    let id = crear_ok(&m, &m.a, m.cat.total(&hiring)).await;
    accion(&m, &m.a, &id, "publish").await;
    m.db.exec(&format!(
        "UPDATE job_offers SET applications_count = 3 WHERE id = '{id}'"
    ))
    .await
    .unwrap();
    assert_eq!(leer(&m, &m.a, &id).await["applications_count"], 3);

    // Editables.
    let mut ok = m.cat.total(&hiring);
    ok["description"] = json!("Una descripción nueva y suficientemente larga.");
    ok["schedule"] = json!("Martes a sábado");
    ok["vacancies"] = json!(5);
    ok["hiring_date"] = json!(fecha(&m.db, 90).await);
    let (e, v) = put(&m, &m.a, &id, ok.clone()).await;
    assert_eq!(e, StatusCode::NO_CONTENT, "{v}");
    let actual = leer(&m, &m.a, &id).await;
    assert_eq!(actual["vacancies"], 5);
    assert_eq!(actual["schedule"], "Martes a sábado");
    // Idempotente.
    assert_eq!(
        put(&m, &m.a, &id, ok.clone()).await.0,
        StatusCode::NO_CONTENT
    );

    // Cada campo congelado, uno por uno.
    let cambios = [
        ("title", json!("Otro título")),
        ("is_experience_required", json!(false)),
        ("required_experience", json!(3)),
        ("main_language_id", json!(m.cat.languages[2])),
        ("required_availability_to_travel", json!(true)),
        ("profession_id", json!(m.cat.skills[0])),
        ("employment_sector_id", Value::Null),
        ("job_hierarchy_id", Value::Null),
        ("minimum_academic_level_id", Value::Null),
        ("salary_range_id", Value::Null),
        ("working_shift_id", Value::Null),
        ("contract_type_id", Value::Null),
        ("skills", json!([])),
        ("languages", json!([])),
        ("driver_license_ids", json!([])),
    ];
    for (campo, valor) in cambios {
        let mut cuerpo = ok.clone();
        cuerpo[campo] = valor;
        if campo == "is_experience_required" {
            cuerpo["required_experience"] = Value::Null;
        }
        let (e, v) = put(&m, &m.a, &id, cuerpo).await;
        // `profession_id` con un id de otro catálogo es 422 (cuerpo) antes que 409.
        if campo == "profession_id" {
            assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{campo}: {v}");
            continue;
        }
        assert_eq!(e, StatusCode::CONFLICT, "{campo}: {v}");
        assert_eq!(codigo(&v), "oferta_con_aplicaciones", "{campo}");
    }
    // Cambiar la profesión por otra válida también es un cambio congelado.
    let otra = texto(
        &m.db,
        &format!(
            "SELECT id::text FROM professions WHERE id <> '{}' ORDER BY code LIMIT 1",
            m.cat.profession
        ),
    )
    .await;
    let mut cuerpo = ok.clone();
    cuerpo["profession_id"] = json!(otra);
    let (e, v) = put(&m, &m.a, &id, cuerpo).await;
    assert_eq!(
        (e, codigo(&v)),
        (StatusCode::CONFLICT, "oferta_con_aplicaciones")
    );
    // Nada de lo congelado cambió.
    let despues = leer(&m, &m.a, &id).await;
    assert_eq!(despues["title"], "Desarrollador backend");
    assert_eq!(despues["skills"].as_array().unwrap().len(), 2);
    assert_eq!(despues["profession_id"], m.cat.profession.as_str());
    m.db.cleanup().await;
}

/// Sin aplicaciones, una publicada se edita entera.
#[tokio::test]
async fn sin_aplicaciones_una_publicada_se_edita_entera() {
    let m = mundo().await;
    let id = publicada(&m, &m.a).await;
    let mut nuevo = m.cat.completa();
    nuevo["title"] = json!("Título nuevo");
    nuevo["skills"] =
        json!([{"skill_id": m.cat.skills[3], "is_required": true, "min_level": "expert"}]);
    assert_eq!(put(&m, &m.a, &id, nuevo).await.0, StatusCode::NO_CONTENT);
    let v = leer(&m, &m.a, &id).await;
    assert_eq!(v["title"], "Título nuevo");
    assert_eq!(v["skills"][0]["skill_id"], m.cat.skills[3].as_str());
    assert_eq!(v["status"], "published");
    m.db.cleanup().await;
}

/// `is_confidential` queda congelado desde la primera publicación (también en pausa), pero un
/// borrador sí lo cambia.
#[tokio::test]
async fn la_confidencialidad_se_congela_al_publicar() {
    let m = mundo().await;
    let id = crear_ok(&m, &m.a, m.cat.completa()).await;
    let mut conf = m.cat.completa();
    conf["is_confidential"] = json!(true);
    assert_eq!(
        put(&m, &m.a, &id, conf.clone()).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(leer(&m, &m.a, &id).await["is_confidential"], true);
    accion(&m, &m.a, &id, "publish").await;
    let (e, v) = put(&m, &m.a, &id, m.cat.completa()).await;
    assert_eq!(e, StatusCode::CONFLICT, "{v}");
    assert_eq!(codigo(&v), "confidencial_inmutable");
    accion(&m, &m.a, &id, "pause").await;
    let (e, v) = put(&m, &m.a, &id, m.cat.completa()).await;
    assert_eq!(e, StatusCode::CONFLICT);
    assert_eq!(codigo(&v), "confidencial_inmutable");
    // Mismo valor: pasa.
    assert_eq!(put(&m, &m.a, &id, conf).await.0, StatusCode::NO_CONTENT);
    assert_eq!(leer(&m, &m.a, &id).await["is_confidential"], true);
    m.db.cleanup().await;
}

/// Un borrador nunca publicado cambia `is_confidential` las veces que quiera.
#[tokio::test]
async fn la_confidencialidad_de_un_borrador_no_se_congela() {
    let m = mundo().await;
    let id = crear_ok(&m, &m.a, m.cat.completa()).await;
    for valor in [true, false, true] {
        let mut c = m.cat.completa();
        c["is_confidential"] = json!(valor);
        assert_eq!(put(&m, &m.a, &id, c).await.0, StatusCode::NO_CONTENT);
    }
    m.db.cleanup().await;
}

/// DELETE: solo un `draft` sin aplicaciones.
#[tokio::test]
async fn borrar_solo_un_borrador_sin_aplicaciones() {
    let m = mundo().await;
    let hiring = fecha(&m.db, 60).await;
    let id = crear_ok(&m, &m.a, m.cat.total(&hiring)).await;
    m.db.exec(&format!(
        "UPDATE job_offers SET applications_count = 1 WHERE id = '{id}'"
    ))
    .await
    .unwrap();
    let ruta = format!("{OFERTAS}/{id}");
    let (e, v) = enviar(&m.app, "DELETE", &ruta, Some(&m.a), None).await;
    assert_eq!((e, codigo(&v)), (StatusCode::CONFLICT, "estado_invalido"));
    m.db.exec(&format!(
        "UPDATE job_offers SET applications_count = 0 WHERE id = '{id}'"
    ))
    .await
    .unwrap();
    let (e, _) = enviar(&m.app, "DELETE", &ruta, Some(&m.a), None).await;
    assert_eq!(e, StatusCode::NO_CONTENT);
    assert_eq!(m.db.count("SELECT count(*) FROM job_offers").await, 0);
    // Sus requisitos se van con ella.
    assert_eq!(m.db.count("SELECT count(*) FROM job_offer_skills").await, 0);
    let (e, _) = enviar(&m.app, "DELETE", &ruta, Some(&m.a), None).await;
    assert_eq!(e, StatusCode::NOT_FOUND);

    let id = publicada(&m, &m.a).await;
    let (e, v) = enviar(
        &m.app,
        "DELETE",
        &format!("{OFERTAS}/{id}"),
        Some(&m.a),
        None,
    )
    .await;
    assert_eq!((e, codigo(&v)), (StatusCode::CONFLICT, "estado_invalido"));
    m.db.cleanup().await;
}

// ================================================================== OF8

/// OF8 + SX2 + SX6: una oferta de otra empresa, una inexistente y un id mal formado dan el
/// mismo 404, con cuerpo idéntico, en TODAS las rutas por id; y la oferta ajena no cambia.
#[tokio::test]
async fn of8_la_oferta_de_otra_empresa_es_404_identico_al_inexistente() {
    let m = mundo().await;
    let ajena = publicada(&m, &m.a).await;
    let borrador = crear_ok(&m, &m.a, m.cat.completa()).await;
    let inexistente = "019a0000-0000-7000-8000-000000000000";
    let rutas: [(&str, &str, Option<Value>); 6] = [
        ("GET", "", None),
        ("PUT", "", Some(m.cat.completa())),
        ("DELETE", "", None),
        ("POST", "/publish", None),
        ("POST", "/pause", None),
        ("POST", "/close", None),
    ];
    let antes = texto(
        &m.db,
        "SELECT string_agg(row(o.*)::text, '|' ORDER BY id) FROM job_offers o",
    )
    .await;
    for (metodo, sufijo, cuerpo) in rutas {
        let mut respuestas = Vec::new();
        for id in [
            ajena.as_str(),
            borrador.as_str(),
            inexistente,
            "no-es-uuid",
            "123",
        ] {
            let (e, v) = enviar(
                &m.app,
                metodo,
                &format!("{OFERTAS}/{id}{sufijo}"),
                Some(&m.b),
                cuerpo.clone(),
            )
            .await;
            respuestas.push((e, v));
        }
        for r in &respuestas {
            assert_eq!(r.0, StatusCode::NOT_FOUND, "{metodo} {sufijo}: {r:?}");
            assert_eq!(r, &respuestas[0], "{metodo} {sufijo}: no son idénticas");
            assert_eq!(codigo(&r.1), "no_encontrado");
        }
    }
    let despues = texto(
        &m.db,
        "SELECT string_agg(row(o.*)::text, '|' ORDER BY id) FROM job_offers o",
    )
    .await;
    assert_eq!(antes, despues, "la empresa B tocó ofertas de A");
    m.db.cleanup().await;
}

/// OF8: el listado trae solo las ofertas de la empresa de quien llama.
#[tokio::test]
async fn el_listado_es_solo_de_mi_empresa() {
    let m = mundo().await;
    let mia = crear_ok(&m, &m.a, m.cat.completa()).await;
    let _ajena = crear_ok(&m, &m.b, m.cat.completa()).await;
    let (e, v) = enviar(&m.app, "GET", OFERTAS, Some(&m.a), None).await;
    assert_eq!(e, StatusCode::OK, "{v}");
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], mia.as_str());
    assert_eq!(v["next_cursor"], Value::Null);
    m.db.cleanup().await;
}

/// Dos reclutadores de la misma empresa sí comparten las ofertas.
#[tokio::test]
async fn los_reclutadores_de_una_empresa_comparten_sus_ofertas() {
    let m = mundo().await;
    let colega = reclutador(
        &m.db,
        &m.app,
        "colega@correo-prueba.com",
        &m.emp_a,
        "recruiter",
    )
    .await;
    let id = crear_ok(&m, &m.a, m.cat.completa()).await;
    let v = leer(&m, &colega, &id).await;
    assert_eq!(v["id"], id.as_str());
    assert_eq!(
        put(&m, &colega, &id, m.cat.completa()).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(accion(&m, &colega, &id, "publish").await.0, StatusCode::OK);
    m.db.cleanup().await;
}

// ================================================================== OF9

/// OF9: la tarea diaria cierra las `published`/`paused` con `hiring_date` pasada y fija
/// `closed_at` con el reloj inyectado; no toca borradores, futuras, hoy ni cerradas.
#[tokio::test]
async fn of9_el_cierre_automatico_usa_el_reloj_inyectado() {
    let m = mundo().await;
    let vencida_pub = publicada(&m, &m.a).await;
    let vencida_pausa = publicada(&m, &m.a).await;
    accion(&m, &m.a, &vencida_pausa, "pause").await;
    let hoy_pub = publicada(&m, &m.a).await;
    let futura = publicada(&m, &m.a).await;
    let borrador = crear_ok(&m, &m.a, m.cat.completa()).await;
    let cerrada = publicada(&m, &m.a).await;
    accion(&m, &m.a, &cerrada, "close").await;
    m.db.exec(&format!(
        "UPDATE job_offers SET hiring_date = DATE '2030-03-09' \
         WHERE id IN ('{vencida_pub}', '{vencida_pausa}', '{borrador}', '{cerrada}')"
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "UPDATE job_offers SET hiring_date = DATE '2030-03-10' WHERE id = '{hoy_pub}'"
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "UPDATE job_offers SET hiring_date = DATE '2030-03-11' WHERE id = '{futura}'"
    ))
    .await
    .unwrap();
    let cierre_manual = texto(
        &m.db,
        &format!("SELECT closed_at::text FROM job_offers WHERE id = '{cerrada}'"),
    )
    .await;

    // «Ahora» = 2030-03-10 15:30:00 UTC (microsegundos Unix).
    let ahora: i64 = 1899387000000000;
    assert!(
        texto(
            &m.db,
            &format!(
                "SELECT (timestamptz 'epoch' + {ahora}::bigint * interval '1 microsecond')::text"
            )
        )
        .await
        .starts_with("2030-03-10 15:30:00"),
        "la constante de la prueba no es 2030-03-10 15:30"
    );
    let n = jobout_core_api::offers::close_expired(&m.db.conn, ahora)
        .await
        .unwrap();
    assert_eq!(n, 2);
    for id in [&vencida_pub, &vencida_pausa] {
        assert_eq!(estado_de(&m, id).await, "closed");
        assert_eq!(
            texto(
                &m.db,
                &format!(
                    "SELECT to_char(closed_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS') \
                     FROM job_offers WHERE id = '{id}'"
                )
            )
            .await,
            "2030-03-10 15:30:00"
        );
    }
    assert_eq!(estado_de(&m, &hoy_pub).await, "published");
    assert_eq!(estado_de(&m, &futura).await, "published");
    assert_eq!(estado_de(&m, &borrador).await, "draft");
    assert_eq!(
        texto(
            &m.db,
            &format!("SELECT closed_at::text FROM job_offers WHERE id = '{cerrada}'")
        )
        .await,
        cierre_manual
    );
    // Idempotente: una segunda pasada no cierra nada.
    assert_eq!(
        jobout_core_api::offers::close_expired(&m.db.conn, ahora)
            .await
            .unwrap(),
        0
    );
    // Al día siguiente cierra la de hoy.
    assert_eq!(
        jobout_core_api::offers::close_expired(&m.db.conn, ahora + 86_400_000_000)
            .await
            .unwrap(),
        1
    );
    assert_eq!(estado_de(&m, &hoy_pub).await, "closed");
    m.db.cleanup().await;
}

/// Cerrar a mano fija `closed_at`, y cerrar no toca otras ofertas.
#[tokio::test]
async fn cerrar_a_mano_fija_closed_at() {
    let m = mundo().await;
    let id = publicada(&m, &m.a).await;
    let otra = publicada(&m, &m.a).await;
    accion(&m, &m.a, &id, "close").await;
    assert!(leer(&m, &m.a, &id).await["closed_at"].is_string());
    assert_eq!(leer(&m, &m.a, &otra).await["closed_at"], Value::Null);
    m.db.cleanup().await;
}

// ============================================================ OF10 (texto)

/// OF10: el texto es dato. El HTML se guarda y se devuelve tal cual (el frontend lo escapa);
/// se recorta, y los caracteres de control y de dirección se eliminan.
#[tokio::test]
async fn of10_el_texto_se_guarda_como_dato() {
    let m = mundo().await;
    let titulo = "  <script>alert(1)</script> & \"comillas\"\u{0000}\u{0007}  ";
    let descripcion =
        "<img src=x onerror=alert(1)>\nSegunda línea\tcon tab\u{202E}oculto\u{200B}\r\n y más texto largo";
    let (e, v) = crear(
        &m,
        &m.a,
        json!({"title": titulo, "description": descripcion, "schedule": "L-V\n8 a 17"}),
    )
    .await;
    assert_eq!(e, StatusCode::CREATED, "{v}");
    assert_eq!(v["title"], "<script>alert(1)</script> & \"comillas\"");
    assert_eq!(
        v["description"],
        "<img src=x onerror=alert(1)>\nSegunda línea\tcon taboculto\n y más texto largo"
    );
    assert_eq!(v["schedule"], "L-V8 a 17");
    m.db.cleanup().await;
}

/// OF10: un JSON roto o de otro tipo es 422 con texto fijo que no repite lo enviado.
#[tokio::test]
async fn el_cuerpo_invalido_es_422_con_texto_fijo() {
    let m = mundo().await;
    for cuerpo in [
        "{\"title\": \"secreto-xyz-77\", ".to_string(),
        "[1,2]".to_string(),
        "\"secreto-xyz-77\"".to_string(),
        String::new(),
        "{\"title\": 5}".to_string(),
        "{\"title\": \"secreto-xyz-77\", \"vacancies\": \"dos\"}".to_string(),
        "{\"title\": \"secreto-xyz-77\", \"skills\": \"secreto-xyz-77\"}".to_string(),
    ] {
        let (e, v, _) = enviar_crudo(&m.app, "POST", OFERTAS, Some(&m.a), Body::from(cuerpo)).await;
        assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
        assert_eq!(codigo(&v), "datos_invalidos");
        assert!(!v.to_string().contains("secreto-xyz-77"));
    }
    m.db.cleanup().await;
}

// ============================================================ OF11 / OF12

/// OF11/OF12: todo `*_id` sale de su catálogo y debe estar activo; desconocido, inactivo,
/// malformado o de otro catálogo → 422 `datos_invalidos`. Ocurre en crear y en editar.
#[tokio::test]
async fn of11_of12_los_ids_salen_de_los_catalogos_activos() {
    let m = mundo().await;
    let desconocido = "019a0000-0000-7000-8000-000000000000";
    let campos = [
        "main_language_id",
        "profession_id",
        "employment_sector_id",
        "job_hierarchy_id",
        "minimum_academic_level_id",
        "state_id",
        "city_id",
        "salary_range_id",
        "working_shift_id",
        "contract_type_id",
    ];
    let hiring = fecha(&m.db, 30).await;
    let id = crear_ok(&m, &m.a, m.cat.total(&hiring)).await;
    for campo in campos {
        for malo in [desconocido, "no-es-uuid", m.cat.skills[0].as_str()] {
            let mut cuerpo = m.cat.total(&hiring);
            cuerpo[campo] = json!(malo);
            let (e, v) = crear(&m, &m.a, cuerpo.clone()).await;
            assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{campo} {malo}: {v}");
            assert_eq!(codigo(&v), "datos_invalidos");
            let (e, _) = put(&m, &m.a, &id, cuerpo).await;
            assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "PUT {campo}");
        }
    }
    // Requisitos: skill, idioma y licencia desconocidos o de otro catálogo.
    for cuerpo in [
        json!({"skills": [{"skill_id": desconocido, "is_required": true}]}),
        json!({"skills": [{"skill_id": m.cat.languages[0], "is_required": true}]}),
        json!({"languages": [{"language_id": desconocido, "min_proficiency": "basic"}]}),
        json!({"languages": [{"language_id": m.cat.skills[0], "min_proficiency": "basic"}]}),
        json!({"driver_license_ids": [desconocido]}),
        json!({"driver_license_ids": [m.cat.skills[0]]}),
        json!({"driver_license_ids": ["x"]}),
    ] {
        let mut c = m.cat.completa();
        for (k, val) in cuerpo.as_object().unwrap() {
            c[k] = val.clone();
        }
        assert_eq!(crear(&m, &m.a, c).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(m.db.count("SELECT count(*) FROM job_offers").await, 1);

    // Inactivos.
    for (tabla, campo, valor) in [
        ("professions", "profession_id", m.cat.profession.clone()),
        ("salary_ranges", "salary_range_id", m.cat.salary.clone()),
        ("states", "state_id", m.cat.state.clone()),
        ("cities", "city_id", m.cat.city.clone()),
        ("languages", "main_language_id", m.cat.languages[0].clone()),
        ("working_shifts", "working_shift_id", m.cat.shift.clone()),
        ("contract_types", "contract_type_id", m.cat.contract.clone()),
        (
            "employment_sectors",
            "employment_sector_id",
            m.cat.sector.clone(),
        ),
        (
            "job_hierarchies",
            "job_hierarchy_id",
            m.cat.hierarchy.clone(),
        ),
        (
            "academic_levels",
            "minimum_academic_level_id",
            m.cat.level.clone(),
        ),
    ] {
        m.db.exec(&format!(
            "UPDATE {tabla} SET active = false WHERE id = '{valor}'"
        ))
        .await
        .unwrap();
        let (e, v) = crear(&m, &m.a, m.cat.total(&hiring)).await;
        assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{campo} inactivo: {v}");
        m.db.exec(&format!(
            "UPDATE {tabla} SET active = true WHERE id = '{valor}'"
        ))
        .await
        .unwrap();
    }
    for (tabla, valor) in [
        ("skills", m.cat.skills[0].clone()),
        ("driver_licenses", m.cat.licenses[0].clone()),
    ] {
        m.db.exec(&format!(
            "UPDATE {tabla} SET active = false WHERE id = '{valor}'"
        ))
        .await
        .unwrap();
        assert_eq!(
            crear(&m, &m.a, m.cat.total(&hiring)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{tabla} inactivo"
        );
        m.db.exec(&format!(
            "UPDATE {tabla} SET active = true WHERE id = '{valor}'"
        ))
        .await
        .unwrap();
    }
    m.db.cleanup().await;
}

/// La ciudad debe ser del estado; una ciudad sin estado también es 422.
#[tokio::test]
async fn la_ciudad_debe_ser_del_estado() {
    let m = mundo().await;
    let mut c = m.cat.completa();
    c["city_id"] = json!(m.cat.otra_ciudad_de_otro_estado);
    let (e, v) = crear(&m, &m.a, c).await;
    assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(codigo(&v), "datos_invalidos");
    let mut c = m.cat.completa();
    c.as_object_mut().unwrap().remove("state_id");
    assert_eq!(crear(&m, &m.a, c).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    m.db.cleanup().await;
}

// ============================================================ límites de campo

/// Límites de cada campo (valores en el borde y uno más allá).
#[tokio::test]
async fn los_limites_de_los_campos() {
    let m = mundo().await;
    let hoy = fecha(&m.db, 0).await;
    let dos_anios = texto(
        &m.db,
        "SELECT ((now() AT TIME ZONE 'UTC')::date + interval '2 years')::date::text",
    )
    .await;
    let pasado_dos_anios = texto(
        &m.db,
        "SELECT ((now() AT TIME ZONE 'UTC')::date + interval '2 years' + interval '1 day')::date::text",
    )
    .await;
    let ayer = fecha(&m.db, -1).await;
    let skills = |n: usize| -> Value {
        Value::Array(
            (0..n)
                .map(|i| json!({"skill_id": m.cat.skills[i], "is_required": true}))
                .collect(),
        )
    };
    let idiomas = |n: usize| -> Value {
        Value::Array(
            (0..n)
                .map(|i| json!({"language_id": m.cat.languages[i], "min_proficiency": "basic"}))
                .collect(),
        )
    };
    let licencias =
        |n: usize| -> Value { Value::Array((0..n).map(|i| json!(m.cat.licenses[i])).collect()) };
    let largo = |c: char, n: usize| -> Value { json!(c.to_string().repeat(n)) };
    // (campo, valor, válido)
    let casos: Vec<(&str, Value, bool)> = vec![
        ("title", largo('a', 2), false),
        ("title", largo('a', 3), true),
        ("title", largo('á', 120), true),
        ("title", largo('á', 121), false),
        ("title", json!("   ab   "), false),
        ("description", largo('d', 19), false),
        ("description", largo('d', 20), true),
        ("description", largo('é', 5000), true),
        ("description", largo('é', 5001), false),
        ("schedule", largo('s', 120), true),
        ("schedule", largo('s', 121), false),
        ("vacancies", json!(0), false),
        ("vacancies", json!(1), true),
        ("vacancies", json!(999), true),
        ("vacancies", json!(1000), false),
        ("vacancies", json!(1.5), false),
        ("hiring_date", json!(hoy), true),
        ("hiring_date", json!(ayer), false),
        ("hiring_date", json!(dos_anios), true),
        ("hiring_date", json!(pasado_dos_anios), false),
        ("hiring_date", json!("2027-02-30"), false),
        ("hiring_date", json!("01/02/2027"), false),
        ("skills", skills(30), true),
        ("skills", skills(31), false),
        (
            "skills",
            json!([{"skill_id": m.cat.skills[0]}, {"skill_id": m.cat.skills[0]}]),
            false,
        ),
        (
            "skills",
            json!([{"skill_id": m.cat.skills[0], "min_level": "master"}]),
            false,
        ),
        (
            "skills",
            json!([{"skill_id": m.cat.skills[0], "min_level": "expert"}]),
            true,
        ),
        ("languages", idiomas(5), true),
        ("languages", idiomas(6), false),
        (
            "languages",
            json!([{"language_id": m.cat.languages[0], "min_proficiency": "fluent"}]),
            false,
        ),
        (
            "languages",
            json!([{"language_id": m.cat.languages[0], "min_proficiency": "native"},
                   {"language_id": m.cat.languages[0], "min_proficiency": "basic"}]),
            false,
        ),
        ("driver_license_ids", licencias(5), true),
        ("driver_license_ids", licencias(6), false),
        (
            "driver_license_ids",
            json!([m.cat.licenses[0], m.cat.licenses[0]]),
            false,
        ),
        ("is_confidential", json!("si"), false),
    ];
    for (campo, valor, valido) in casos {
        let mut c = m.cat.completa();
        c[campo] = valor.clone();
        let (e, v) = crear(&m, &m.a, c).await;
        let resumen = valor.to_string().chars().take(60).collect::<String>();
        if valido {
            assert_eq!(e, StatusCode::CREATED, "{campo}={resumen}: {v}");
        } else {
            assert_eq!(
                e,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{campo}={resumen}: {v}"
            );
            assert_eq!(codigo(&v), "datos_invalidos", "{campo}={resumen}");
        }
        m.db.exec("DELETE FROM job_offers").await.unwrap();
    }
    m.db.cleanup().await;
}

/// La experiencia: `required_experience` 0–50, obligatoria si se exige y nula si no.
#[tokio::test]
async fn la_experiencia_requerida() {
    let m = mundo().await;
    let casos = [
        (json!(true), json!(0), true),
        (json!(true), json!(50), true),
        (json!(true), json!(51), false),
        (json!(true), json!(-1), false),
        (json!(true), Value::Null, false),
        (json!(false), Value::Null, true),
        (json!(false), json!(2), false),
        (Value::Null, Value::Null, true),
        (Value::Null, json!(2), false),
    ];
    for (exigida, anios, valido) in casos {
        let mut c = m.cat.completa();
        c["is_experience_required"] = exigida.clone();
        c["required_experience"] = anios.clone();
        let (e, v) = crear(&m, &m.a, c).await;
        let esperado = if valido {
            StatusCode::CREATED
        } else {
            StatusCode::UNPROCESSABLE_ENTITY
        };
        assert_eq!(e, esperado, "{exigida} {anios}: {v}");
    }
    m.db.cleanup().await;
}

// ============================================================ tope de cuerpo

/// 64 KB en `POST`/`PUT` de ofertas; pasarlos es 413 (con cuerpo JSON), antes que cualquier 422;
/// 8 KB en las acciones.
#[tokio::test]
async fn el_tope_de_cuerpo_es_de_64_kb() {
    let m = mundo().await;
    let id = crear_ok(&m, &m.a, m.cat.completa()).await;
    // 20 KB: pasa el tope de la ruta (64 KB) aunque supere el global (8 KB); falla por campo
    // desconocido, no por tamaño.
    let relleno = json!({"title": "Un título", "padding_de_prueba": "y".repeat(20_000)});
    let (e, v, _) = enviar_crudo(
        &m.app,
        "POST",
        OFERTAS,
        Some(&m.a),
        Body::from(relleno.to_string()),
    )
    .await;
    assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(codigo(&v), "campo_no_permitido");

    let enorme = json!({"title": "Un título", "padding_de_prueba": "y".repeat(70_000)});
    for (metodo, ruta) in [
        ("POST", OFERTAS.to_string()),
        ("PUT", format!("{OFERTAS}/{id}")),
    ] {
        let (e, v, _) = enviar_crudo(
            &m.app,
            metodo,
            &ruta,
            Some(&m.a),
            Body::from(enorme.to_string()),
        )
        .await;
        assert_eq!(e, StatusCode::PAYLOAD_TOO_LARGE, "{metodo}: {v}");
        assert_eq!(codigo(&v), "cuerpo_demasiado_grande");
    }
    // Las acciones usan el global de 8 KB.
    let (e, _, _) = enviar_crudo(
        &m.app,
        "POST",
        &format!("{OFERTAS}/{id}/publish"),
        Some(&m.a),
        Body::from("x".repeat(9_000)),
    )
    .await;
    assert_eq!(e, StatusCode::PAYLOAD_TOO_LARGE);
    m.db.cleanup().await;
}

// ============================================================ 401 / 403 / roles

fn todas_las_rutas(id: &str) -> Vec<(&'static str, String, &'static str)> {
    // (método, ruta, permiso)
    vec![
        ("POST", OFERTAS.to_string(), "offer.create"),
        ("GET", OFERTAS.to_string(), "offer.update"),
        ("GET", format!("{OFERTAS}/{id}"), "offer.update"),
        ("PUT", format!("{OFERTAS}/{id}"), "offer.update"),
        ("DELETE", format!("{OFERTAS}/{id}"), "offer.update"),
        ("POST", format!("{OFERTAS}/{id}/publish"), "offer.publish"),
        ("POST", format!("{OFERTAS}/{id}/pause"), "offer.publish"),
        ("POST", format!("{OFERTAS}/{id}/close"), "offer.close"),
    ]
}

/// SX3: sin token (o con basura) → 401 en todas las rutas.
#[tokio::test]
async fn sx3_sin_token_es_401_en_todas_las_rutas() {
    let m = mundo().await;
    let id = crear_ok(&m, &m.a, m.cat.completa()).await;
    for (metodo, ruta, _) in todas_las_rutas(&id) {
        for auth in [None, Some("x.y.z")] {
            let (e, v) = enviar(&m.app, metodo, &ruta, auth, None).await;
            assert_eq!(e, StatusCode::UNAUTHORIZED, "{metodo} {ruta}");
            assert_eq!(codigo(&v), "no_autenticado");
        }
    }
    m.db.cleanup().await;
}

/// RO1: un candidato (sin permisos) recibe 403 `sin_permiso` en todas las rutas, aunque el
/// recurso exista y sea de otra empresa.
#[tokio::test]
async fn ro1_un_candidato_es_403_sin_permiso() {
    let m = mundo().await;
    let id = crear_ok(&m, &m.a, m.cat.completa()).await;
    let (cand, _) = registrar(&m.app, "cand@correo-prueba.com").await;
    for (metodo, ruta, _) in todas_las_rutas(&id) {
        let (e, v) = enviar(&m.app, metodo, &ruta, Some(&cand), Some(m.cat.completa())).await;
        assert_eq!(e, StatusCode::FORBIDDEN, "{metodo} {ruta}");
        assert_eq!(codigo(&v), "sin_permiso");
    }
    m.db.cleanup().await;
}

/// Cada ruta exige SU permiso y solo ese: un rol con todos los permisos de oferta salvo uno
/// recibe 403 exactamente en las rutas de ese permiso.
#[tokio::test]
async fn cada_ruta_exige_su_permiso() {
    let m = mundo().await;
    let id = crear_ok(&m, &m.a, m.cat.completa()).await;
    for faltante in [
        "offer.create",
        "offer.update",
        "offer.publish",
        "offer.close",
    ] {
        let rol = format!("solo-sin-{faltante}");
        m.db.exec(&format!(
            "INSERT INTO roles (id, code, name) VALUES (gen_random_uuid(), '{rol}', '{rol}')"
        ))
        .await
        .unwrap();
        m.db.exec(&format!(
            "INSERT INTO role_permissions (role_id, permission_id) \
             SELECT (SELECT id FROM roles WHERE code = '{rol}'), id FROM permissions \
             WHERE code LIKE 'offer.%' AND code <> '{faltante}'"
        ))
        .await
        .unwrap();
        let email = format!("{faltante}@correo-prueba.com");
        let (token, uid) = registrar(&m.app, &email).await;
        miembro(&m.db, &uid, &m.emp_a, &rol).await;
        for (metodo, ruta, permiso) in todas_las_rutas(&id) {
            let (e, v) = enviar(&m.app, metodo, &ruta, Some(&token), Some(m.cat.completa())).await;
            if permiso == faltante {
                assert_eq!(e, StatusCode::FORBIDDEN, "{metodo} {ruta} sin {faltante}");
                assert_eq!(codigo(&v), "sin_permiso");
            } else {
                assert_ne!(
                    e,
                    StatusCode::FORBIDDEN,
                    "{metodo} {ruta} con {permiso}: {v}"
                );
            }
        }
    }
    m.db.cleanup().await;
}

/// RO4: quitar el rol corta el acceso en la petición siguiente.
#[tokio::test]
async fn ro4_quitar_el_rol_corta_el_acceso() {
    let m = mundo().await;
    assert_eq!(
        enviar(&m.app, "GET", OFERTAS, Some(&m.a), None).await.0,
        StatusCode::OK
    );
    m.db.exec(
        "DELETE FROM user_roles WHERE role_id = (SELECT id FROM roles WHERE code = 'company_admin')",
    )
    .await
    .unwrap();
    let (e, v) = enviar(&m.app, "GET", OFERTAS, Some(&m.a), None).await;
    assert_eq!((e, codigo(&v)), (StatusCode::FORBIDDEN, "sin_permiso"));
    m.db.cleanup().await;
}

/// Una cuenta suspendida: 403 `cuenta_no_disponible`, antes que `sin_permiso`.
#[tokio::test]
async fn una_cuenta_suspendida_es_cuenta_no_disponible() {
    let m = mundo().await;
    m.db.exec("UPDATE users SET status = 'suspended'")
        .await
        .unwrap();
    let (e, v) = enviar(&m.app, "GET", OFERTAS, Some(&m.a), None).await;
    assert_eq!(
        (e, codigo(&v)),
        (StatusCode::FORBIDDEN, "cuenta_no_disponible")
    );
    m.db.cleanup().await;
}

/// Con el permiso pero sin empresa (rol sin fila, o fila dada de baja) → 404 `no_encontrado`
/// en todas las rutas.
#[tokio::test]
async fn con_permiso_pero_sin_empresa_es_404() {
    let m = mundo().await;
    let id = crear_ok(&m, &m.a, m.cat.completa()).await;
    let (sin, uid) = registrar(&m.app, "sin@correo-prueba.com").await;
    dar_rol(&m.db, &uid, "recruiter").await; // el rol sin fila en company_recruiters no sirve
    for (metodo, ruta, _) in todas_las_rutas(&id) {
        let (e, v) = enviar(&m.app, metodo, &ruta, Some(&sin), Some(m.cat.completa())).await;
        assert_eq!(e, StatusCode::NOT_FOUND, "{metodo} {ruta}: {v}");
        assert_eq!(codigo(&v), "no_encontrado");
    }
    // Un reclutador dado de baja (deleted_at) tampoco.
    m.db.exec(&format!(
        "UPDATE company_recruiters SET deleted_at = now() WHERE user_id = '{}'",
        jwt().validate(&m.a).unwrap()
    ))
    .await
    .unwrap();
    assert_eq!(
        enviar(&m.app, "GET", OFERTAS, Some(&m.a), None).await.0,
        StatusCode::NOT_FOUND
    );
    m.db.cleanup().await;
}

// ============================================================ verificación / membresía

/// EM6/EM7: sin verificar se crean, editan y borran borradores, pero `publish` es 403
/// `empresa_no_verificada`; pausar y cerrar siguen permitidos.
#[tokio::test]
async fn una_empresa_sin_verificar_no_publica() {
    let m = mundo().await;
    let id = publicada(&m, &m.a).await;
    m.db.exec(&format!(
        "UPDATE companies SET verified_at = NULL WHERE id = '{}'",
        m.emp_a
    ))
    .await
    .unwrap();
    // Borradores: crear, editar, listar.
    let borrador = crear_ok(&m, &m.a, m.cat.completa()).await;
    assert_eq!(
        put(&m, &m.a, &borrador, m.cat.completa()).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        enviar(&m.app, "GET", OFERTAS, Some(&m.a), None).await.0,
        StatusCode::OK
    );
    // Publicar un borrador y reanudar una pausada: 403.
    let (e, v) = accion(&m, &m.a, &borrador, "publish").await;
    assert_eq!(
        (e, codigo(&v)),
        (StatusCode::FORBIDDEN, "empresa_no_verificada")
    );
    assert_eq!(estado_de(&m, &borrador).await, "draft");
    assert_eq!(
        accion(&m, &m.a, &id, "pause").await.0,
        StatusCode::NO_CONTENT
    );
    let (e, v) = accion(&m, &m.a, &id, "publish").await;
    assert_eq!(
        (e, codigo(&v)),
        (StatusCode::FORBIDDEN, "empresa_no_verificada")
    );
    assert_eq!(estado_de(&m, &id).await, "paused");
    // Cerrar sí; borrar un borrador sí.
    assert_eq!(
        accion(&m, &m.a, &id, "close").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        enviar(
            &m.app,
            "DELETE",
            &format!("{OFERTAS}/{borrador}"),
            Some(&m.a),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    m.db.cleanup().await;
}

/// Membresía vencida, cancelada o inexistente: `publish` es 403 `membresia_vencida`; se renueva
/// y vuelve. Si además no está verificada, gana `empresa_no_verificada`.
#[tokio::test]
async fn una_membresia_vencida_no_publica() {
    let m = mundo().await;
    let vencida = empresa(&m.db, "Vencida", true, Membresia::Vencida).await;
    let sin_membresia = empresa(&m.db, "Sin membresía", true, Membresia::Ninguna).await;
    let ambas = empresa(&m.db, "Ambas", false, Membresia::Vencida).await;
    for (i, (emp, esperado)) in [
        (vencida, "membresia_vencida"),
        (sin_membresia, "membresia_vencida"),
        (ambas, "empresa_no_verificada"),
    ]
    .iter()
    .enumerate()
    {
        let t = reclutador(
            &m.db,
            &m.app,
            &format!("v{i}@correo-prueba.com"),
            emp,
            "company_admin",
        )
        .await;
        let id = crear_ok(&m, &t, m.cat.completa()).await;
        let (e, v) = accion(&m, &t, &id, "publish").await;
        assert_eq!((e, codigo(&v)), (StatusCode::FORBIDDEN, *esperado), "{i}");
        assert_eq!(estado_de(&m, &id).await, "draft");
    }
    // Renovar: la vigente vuelve a publicar.
    let t = reclutador(
        &m.db,
        &m.app,
        "renov@correo-prueba.com",
        &m.emp_a,
        "recruiter",
    )
    .await;
    let id = crear_ok(&m, &t, m.cat.completa()).await;
    m.db.exec(&format!(
        "UPDATE memberships SET expiration_date = current_date - 1, start_date = current_date - 50 \
         WHERE company_id = '{}'",
        m.emp_a
    ))
    .await
    .unwrap();
    let (e, v) = accion(&m, &t, &id, "publish").await;
    assert_eq!(
        (e, codigo(&v)),
        (StatusCode::FORBIDDEN, "membresia_vencida")
    );
    // El estado cancelado tampoco vale, aunque las fechas sirvan.
    m.db.exec(&format!(
        "UPDATE memberships SET expiration_date = current_date + 5, status = 'cancelled' \
         WHERE company_id = '{}'",
        m.emp_a
    ))
    .await
    .unwrap();
    assert_eq!(
        accion(&m, &t, &id, "publish").await.0,
        StatusCode::FORBIDDEN
    );
    m.db.exec(&format!(
        "UPDATE memberships SET status = 'active' WHERE company_id = '{}'",
        m.emp_a
    ))
    .await
    .unwrap();
    assert_eq!(accion(&m, &t, &id, "publish").await.0, StatusCode::OK);
    // Aún vencida, pausar y cerrar se permiten.
    m.db.exec(&format!(
        "UPDATE memberships SET expiration_date = current_date - 1 WHERE company_id = '{}'",
        m.emp_a
    ))
    .await
    .unwrap();
    assert_eq!(accion(&m, &t, &id, "pause").await.0, StatusCode::NO_CONTENT);
    assert_eq!(accion(&m, &t, &id, "close").await.0, StatusCode::NO_CONTENT);
    m.db.cleanup().await;
}

/// Orden de `publish`: 404 → 409 → 403 empresa → 422 incompleta.
#[tokio::test]
async fn el_orden_de_validacion_de_publish() {
    let m = mundo().await;
    let incompleta = crear_ok(&m, &m.a, json!({"title": "Incompleta"})).await;
    m.db.exec(&format!(
        "UPDATE companies SET verified_at = NULL WHERE id = '{}'",
        m.emp_a
    ))
    .await
    .unwrap();
    // 403 gana a 422.
    let (e, v) = accion(&m, &m.a, &incompleta, "publish").await;
    assert_eq!(
        (e, codigo(&v)),
        (StatusCode::FORBIDDEN, "empresa_no_verificada")
    );
    // 409 gana a 403.
    m.db.exec(&format!(
        "UPDATE job_offers SET status = 'closed', closed_at = now() WHERE id = '{incompleta}'"
    ))
    .await
    .unwrap();
    let (e, v) = accion(&m, &m.a, &incompleta, "publish").await;
    assert_eq!((e, codigo(&v)), (StatusCode::CONFLICT, "estado_invalido"));
    // 404 gana a todo para otra empresa.
    let (e, v) = accion(&m, &m.b, &incompleta, "publish").await;
    assert_eq!((e, codigo(&v)), (StatusCode::NOT_FOUND, "no_encontrado"));
    m.db.cleanup().await;
}

// ============================================================ SX5: límite

/// SX5: 20 ofertas por empresa y hora; la 21 es 429 con `Retry-After`; las de hace más de una
/// hora no cuentan; otra empresa no se ve afectada; el cupo es de la empresa, no de la persona.
#[tokio::test]
async fn sx5_veinte_ofertas_por_empresa_y_hora() {
    let m = mundo().await;
    for i in 0..20 {
        let (e, v) = crear(&m, &m.a, json!({"title": format!("Oferta {i}")})).await;
        assert_eq!(e, StatusCode::CREATED, "{i}: {v}");
    }
    let (e, v, h) = enviar_crudo(
        &m.app,
        "POST",
        OFERTAS,
        Some(&m.a),
        Body::from(json!({"title": "La 21"}).to_string()),
    )
    .await;
    assert_eq!(e, StatusCode::TOO_MANY_REQUESTS, "{v}");
    assert_eq!(codigo(&v), "demasiados_intentos");
    let espera: i64 = h
        .get("retry-after")
        .expect("Retry-After")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=3600).contains(&espera), "{espera}");
    assert_eq!(m.db.count("SELECT count(*) FROM job_offers").await, 20);
    // Otra empresa, no.
    assert_eq!(
        crear(&m, &m.b, json!({"title": "De B"})).await.0,
        StatusCode::CREATED
    );
    // Un reclutador distinto de la misma empresa comparte el cupo.
    let colega = reclutador(&m.db, &m.app, "c2@correo-prueba.com", &m.emp_a, "recruiter").await;
    assert_eq!(
        crear(&m, &colega, json!({"title": "Xyz"})).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Una sale de la ventana de una hora: se puede crear exactamente una más.
    let id = texto(
        &m.db,
        &format!(
            "SELECT id::text FROM job_offers WHERE company_id = '{}' LIMIT 1",
            m.emp_a
        ),
    )
    .await;
    m.db.exec(&format!(
        "UPDATE job_offers SET created_at = now() - interval '61 minutes' WHERE id = '{id}'"
    ))
    .await
    .unwrap();
    assert_eq!(
        crear(&m, &m.a, json!({"title": "Una más"})).await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        crear(&m, &m.a, json!({"title": "Otra"})).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Publicar no consume (ni exige) cupo.
    let borrador = texto(
        &m.db,
        &format!(
            "SELECT id::text FROM job_offers WHERE company_id = '{}' LIMIT 1",
            m.emp_b
        ),
    )
    .await;
    m.db.exec(&format!(
        "UPDATE job_offers SET name = 'Publicable', description = 'Una descripción suficientemente larga.', \
         vacancies = 1, profession_id = '{}', state_id = '{}', city_id = '{}' WHERE id = '{borrador}'",
        m.cat.profession, m.cat.state, m.cat.city
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "INSERT INTO job_offer_driver_licenses (job_offer_id, driver_license_id) VALUES ('{borrador}', '{}')",
        m.cat.licenses[0]
    ))
    .await
    .unwrap();
    assert_eq!(
        accion(&m, &m.b, &borrador, "publish").await.0,
        StatusCode::OK
    );
    m.db.cleanup().await;
}

/// Un cuerpo inválido no consume cupo y se valida antes del 429.
#[tokio::test]
async fn un_cuerpo_invalido_no_consume_cupo() {
    let m = mundo().await;
    for _ in 0..25 {
        assert_eq!(
            crear(&m, &m.a, json!({"title": "x"})).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    for i in 0..20 {
        assert_eq!(
            crear(&m, &m.a, json!({"title": format!("Oferta {i}")}))
                .await
                .0,
            StatusCode::CREATED
        );
    }
    // Con el cupo lleno, un cuerpo inválido sigue siendo 422 (validación antes que 429).
    assert_eq!(
        crear(&m, &m.a, json!({"title": "x"})).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    m.db.cleanup().await;
}

/// 30 creaciones simultáneas contra un cupo de 20: pasan exactamente 20.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn las_creaciones_simultaneas_respetan_el_cupo() {
    let m = mundo().await;
    let futuros: Vec<_> = (0..30)
        .map(|i| crear(&m, &m.a, json!({"title": format!("Oferta {i}")})))
        .collect();
    let resultados = futures_util::future::join_all(futuros).await;
    let creadas = resultados
        .iter()
        .filter(|r| r.0 == StatusCode::CREATED)
        .count();
    assert_eq!(creadas, 20);
    assert!(resultados
        .iter()
        .all(|r| r.0 == StatusCode::CREATED || r.0 == StatusCode::TOO_MANY_REQUESTS));
    m.db.cleanup().await;
}

// ============================================================ listado

/// Paginación por cursor, filtro por estado y validación de la consulta.
#[tokio::test]
async fn el_listado_pagina_y_filtra() {
    let m = mundo().await;
    for i in 0..5 {
        crear_ok(&m, &m.a, json!({"title": format!("Oferta {i}")})).await;
    }
    let pub_id = publicada(&m, &m.a).await;
    let mut vistos = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let uri = match &cursor {
            None => format!("{OFERTAS}?limit=2"),
            Some(c) => format!("{OFERTAS}?limit=2&cursor={c}"),
        };
        let (e, v) = enviar(&m.app, "GET", &uri, Some(&m.a), None).await;
        assert_eq!(e, StatusCode::OK, "{v}");
        let items = v["items"].as_array().unwrap();
        assert!(items.len() <= 2);
        vistos.extend(items.iter().map(|i| i["id"].as_str().unwrap().to_string()));
        for i in items {
            for k in [
                "id",
                "title",
                "status",
                "is_confidential",
                "vacancies",
                "publication_date",
                "applications_count",
                "created_at",
            ] {
                assert!(i.get(k).is_some(), "falta {k}");
            }
        }
        match v["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    assert_eq!(vistos.len(), 6);
    let mut unicos = vistos.clone();
    unicos.sort();
    unicos.dedup();
    assert_eq!(unicos.len(), 6, "hay repetidos");
    assert_eq!(vistos[0], pub_id, "las más nuevas primero");

    let (_, v) = enviar(
        &m.app,
        "GET",
        &format!("{OFERTAS}?status=published"),
        Some(&m.a),
        None,
    )
    .await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let (_, v) = enviar(
        &m.app,
        "GET",
        &format!("{OFERTAS}?status=draft&limit=50"),
        Some(&m.a),
        None,
    )
    .await;
    assert_eq!(v["items"].as_array().unwrap().len(), 5);

    for q in [
        "limit=0",
        "limit=51",
        "limit=x",
        "limit=",
        "status=borrado",
        "status=",
        "cursor=basura",
        "cursor=",
        "limit=1&limit=2",
    ] {
        let (e, v) = enviar(&m.app, "GET", &format!("{OFERTAS}?{q}"), Some(&m.a), None).await;
        assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY, "{q}: {v}");
        assert_eq!(codigo(&v), "datos_invalidos", "{q}");
    }
    m.db.cleanup().await;
}

/// CA7: el listado hace un número fijo de consultas, sin importar cuántas ofertas haya.
#[tokio::test]
async fn el_listado_no_hace_n_mas_1() {
    let db = TestDb::migrated().await;
    let contador = Arc::new(Mutex::new(0usize));
    let c2 = contador.clone();
    let mut conn = db.conn.clone();
    conn.set_metric_callback(move |_| {
        *c2.lock().unwrap() += 1;
    });
    let app = app_with_state(AppState {
        db: conn,
        ..estado(&db)
    });
    let emp = empresa(&db, "Empresa A", true, Membresia::Vigente).await;
    let token = reclutador(&db, &app, "a@correo-prueba.com", &emp, "company_admin").await;
    let cat = Cat::cargar(&db).await;
    let m = Mundo {
        db,
        app,
        cat,
        emp_a: emp,
        emp_b: String::new(),
        a: token.clone(),
        b: String::new(),
    };
    let mut medidas = Vec::new();
    for tanda in [1, 12] {
        for _ in 0..tanda {
            let hiring = fecha(&m.db, 40).await;
            crear_ok(&m, &token, m.cat.total(&hiring)).await;
        }
        *contador.lock().unwrap() = 0;
        let (e, v) = enviar(
            &m.app,
            "GET",
            &format!("{OFERTAS}?limit=50"),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(e, StatusCode::OK);
        assert!(v["items"].as_array().unwrap().len() >= tanda);
        medidas.push(*contador.lock().unwrap());
    }
    assert_eq!(
        medidas[0], medidas[1],
        "el listado depende del número de ofertas: {medidas:?}"
    );
    // Y el detalle, acotado.
    let id = texto(&m.db, "SELECT id::text FROM job_offers LIMIT 1").await;
    *contador.lock().unwrap() = 0;
    leer(&m, &token, &id).await;
    let detalle = *contador.lock().unwrap();
    assert!(detalle <= 8, "el detalle hizo {detalle} consultas");
    m.db.cleanup().await;
}

// ============================================================ PUT: idempotencia

/// `PUT` es reemplazo completo e idempotente; lo omitido se vacía.
#[tokio::test]
async fn put_es_reemplazo_completo_e_idempotente() {
    let m = mundo().await;
    let hiring = fecha(&m.db, 60).await;
    let id = crear_ok(&m, &m.a, m.cat.total(&hiring)).await;
    let antes = leer(&m, &m.a, &id).await;
    let cuerpo = m.cat.total(&hiring);
    assert_eq!(
        put(&m, &m.a, &id, cuerpo.clone()).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(put(&m, &m.a, &id, cuerpo).await.0, StatusCode::NO_CONTENT);
    let despues = leer(&m, &m.a, &id).await;
    assert_eq!(antes["skills"], despues["skills"]);
    assert_eq!(antes["created_at"], despues["created_at"]);
    // Reemplazo: lo omitido se vacía.
    assert_eq!(
        put(&m, &m.a, &id, json!({"title": "Solo título"})).await.0,
        StatusCode::NO_CONTENT
    );
    let v = leer(&m, &m.a, &id).await;
    assert_eq!(v["description"], Value::Null);
    assert_eq!(v["hiring_date"], Value::Null);
    assert_eq!(v["skills"], json!([]));
    assert_eq!(v["state_id"], Value::Null);
    assert_eq!(v["status"], "draft");
    // Un borrador con una fecha de contratación que ya venció se reenvía sin cambios.
    m.db.exec(&format!(
        "UPDATE job_offers SET hiring_date = current_date - 3 WHERE id = '{id}'"
    ))
    .await
    .unwrap();
    let mut c = json!({"title": "Solo título"});
    c["hiring_date"] = json!(fecha(&m.db, -3).await);
    assert_eq!(put(&m, &m.a, &id, c).await.0, StatusCode::NO_CONTENT);
    m.db.cleanup().await;
}

/// El 422 del cuerpo va antes que el 404 del recurso.
#[tokio::test]
async fn el_orden_de_validacion_de_put() {
    let m = mundo().await;
    let ajena = crear_ok(&m, &m.a, m.cat.completa()).await;
    let (e, _) = put(&m, &m.b, &ajena, json!({"title": "x"})).await;
    assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY);
    let (e, _) = put(&m, &m.b, &ajena, m.cat.completa()).await;
    assert_eq!(e, StatusCode::NOT_FOUND);
    m.db.cleanup().await;
}

// ============================================================ SX4: logs

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

/// Ni el texto de una oferta ni el nombre de una empresa confidencial llegan a un log o a un
/// error, en éxito y en fallo. La captura se comprueba con una marca propia y con un evento que
/// SÍ escribe el marcador por el mismo canal (cordura).
#[tokio::test]
async fn sx4_el_texto_de_las_ofertas_no_llega_a_los_logs() {
    let registro = Registro::default();
    let escritor = registro.clone();
    let _guarda = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_env_filter("jobout_core_api=debug,tower_http=debug")
            .with_ansi(false)
            .with_writer(move || escritor.clone())
            .finish(),
    );
    tracing::info!(target: "jobout_core_api::test", "captura-activa");
    let marcador = "marcador-secreto-ofe-7731";

    let m = mundo().await;
    let mut cuerpo = m.cat.completa();
    cuerpo["title"] = json!(format!("Cargo {marcador}"));
    cuerpo["description"] = json!(format!("Descripción larga con {marcador} dentro del texto"));
    cuerpo["schedule"] = json!(marcador);
    cuerpo["is_confidential"] = json!(true);
    let (e, v) = crear(&m, &m.a, cuerpo.clone()).await;
    assert_eq!(e, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(
        put(&m, &m.a, &id, cuerpo.clone()).await.0,
        StatusCode::NO_CONTENT
    );
    leer(&m, &m.a, &id).await;
    accion(&m, &m.a, &id, "publish").await;
    enviar(&m.app, "GET", OFERTAS, Some(&m.a), None).await;
    accion(&m, &m.a, &id, "pause").await;
    accion(&m, &m.a, &id, "close").await;
    // Fallos que tocan el marcador.
    let mut malo = cuerpo.clone();
    malo["vacancies"] = json!(0);
    assert_eq!(
        crear(&m, &m.a, malo).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut raro = cuerpo.clone();
    raro[marcador] = json!(1);
    let (e, v) = crear(&m, &m.a, raro).await;
    assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!v.to_string().contains(marcador));
    let (e, v, _) = enviar_crudo(
        &m.app,
        "POST",
        OFERTAS,
        Some(&m.a),
        Body::from(format!("{{\"title\": \"{marcador}\", ")),
    )
    .await;
    assert_eq!(e, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!v.to_string().contains(marcador));
    let (e, v) = put(&m, &m.b, &id, cuerpo).await; // 404 de otra empresa
    assert_eq!(e, StatusCode::NOT_FOUND);
    assert!(!v.to_string().contains(marcador));
    m.db.cleanup().await;

    let log = registro.texto();
    assert!(log.contains("captura-activa"), "la captura no funciona");
    assert!(!log.contains(marcador), "el log filtra texto de una oferta");
    assert!(!log.contains("Descripción larga"));
    // Cordura: el marcador SÍ se vería si algo lo escribiera por este canal.
    tracing::info!(target: "jobout_core_api::test", "{marcador}");
    assert!(
        registro.texto().contains(marcador),
        "el log no captura el marcador"
    );
}

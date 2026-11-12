//! Casos de verdad de las rutas de perfil (P1–P9), de la revisión de la extracción (V1–V12) y de
//! seguridad (S1–S4) de docs/working/fase2-perfil-cv.md. Contra PostgreSQL real; una base por
//! prueba. Los CV y las extracciones se insertan por SQL: no dependen de las rutas de subida.
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
use common::{bytes_de, contiene, TestDb};
use http_body_util::BodyExt;
use jobout_core_api::{app_with_state, crypto::PiiKeys, jwt::JwtKeys, state::AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

const CLAVE: &str = "una-clave-larga-123";
const PERFIL: &str = "/api/v1/profile";
const TELEFONO: &str = "+52 (555) 123-4567";
const SECRETO: &str = "texto-secreto-de-la-experiencia-9f3a";

fn pii() -> Arc<PiiKeys> {
    Arc::new(
        PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
    )
}

fn app(db: &TestDb) -> Router {
    app_with_state(AppState {
        db: db.conn.clone(),
        pii: pii(),
        jwt: Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap()),
        blobs: jobout_core_api::blob::memory_store(),
        engine: jobout_core_api::interview::default_engine(),
    })
}

async fn enviar_crudo(
    app: &Router,
    metodo: &str,
    uri: &str,
    token: Option<&str>,
    cuerpo: Body,
) -> (StatusCode, Value) {
    let mut p = Request::builder()
        .method(metodo)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = token {
        p = p.header("authorization", format!("Bearer {t}"));
    }
    let r = app.clone().oneshot(p.body(cuerpo).unwrap()).await.unwrap();
    let estado = r.status();
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    (
        estado,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
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
    enviar_crudo(app, metodo, uri, token, cuerpo).await
}

async fn registrar(app: &Router, email: &str) -> String {
    let (estado, cuerpo) = enviar(
        app,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(json!({"email": email, "password": CLAVE, "first_name": "Ana", "last_name": "Paz"})),
    )
    .await;
    assert_eq!(estado, StatusCode::CREATED, "{cuerpo}");
    cuerpo["access_token"].as_str().unwrap().to_string()
}

async fn con_cuenta() -> (TestDb, Router, String) {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let token = registrar(&app, "ana@correo-prueba.com").await;
    (db, app, token)
}

async fn put(app: &Router, token: &str, cuerpo: Value) -> (StatusCode, Value) {
    enviar(app, "PUT", PERFIL, Some(token), Some(cuerpo)).await
}

async fn get_perfil(app: &Router, token: &str) -> Value {
    let (estado, cuerpo) = enviar(app, "GET", PERFIL, Some(token), None).await;
    assert_eq!(estado, StatusCode::OK, "{cuerpo}");
    cuerpo
}

/// Los `n` primeros ids de un catálogo (activos).
async fn ids(db: &TestDb, tabla: &str, n: usize) -> Vec<String> {
    use sea_orm::{ConnectionTrait, Statement};
    let filas = db
        .conn
        .query_all(Statement::from_string(
            db.conn.get_database_backend(),
            format!("SELECT id::text FROM {tabla} WHERE active ORDER BY code LIMIT {n}"),
        ))
        .await
        .unwrap();
    assert_eq!(
        filas.len(),
        n,
        "el catálogo {tabla} tiene menos de {n} valores"
    );
    filas
        .iter()
        .map(|f| f.try_get_by_index(0).unwrap())
        .collect()
}

/// Los n primeros ids de skills (para topes y repetidos).
async fn skill(db: &TestDb, n: usize) -> String {
    common::uuid_de(
        db,
        &format!("SELECT id FROM skills ORDER BY code OFFSET {n} LIMIT 1"),
    )
    .await
    .to_string()
}

async fn idioma(db: &TestDb, n: usize) -> String {
    common::uuid_de(
        db,
        &format!("SELECT id FROM languages ORDER BY code OFFSET {n} LIMIT 1"),
    )
    .await
    .to_string()
}

fn experiencia(company: &str) -> Value {
    json!({"company": company, "title": "Dev", "start_date": "2021-01-01",
           "end_date": null, "description": null})
}

/// Un perfil completo y válido.
async fn perfil_completo(db: &TestDb) -> Value {
    json!({
        "description": "Desarrolladora de software",
        "phone": TELEFONO,
        "academic_level_id": common::uuid_de(db, "SELECT id FROM academic_levels ORDER BY sort_order LIMIT 1").await.to_string(),
        "profession_id": common::uuid_de(db, "SELECT id FROM professions ORDER BY name LIMIT 1").await.to_string(),
        "job_hierarchy_id": common::uuid_de(db, "SELECT id FROM job_hierarchies ORDER BY sort_order LIMIT 1").await.to_string(),
        "skills": [
            {"skill_id": skill(db, 0).await, "level": "advanced", "years_of_experience": 3},
            {"skill_id": skill(db, 1).await, "level": null, "years_of_experience": null},
        ],
        "languages": [{"language_id": idioma(db, 0).await, "proficiency": "native"}],
        "experiences": [
            {"company": "Acme", "title": "Dev", "start_date": "2021-01-01",
             "end_date": "2022-06-30", "description": SECRETO},
            {"company": "Beta", "title": "Lead", "start_date": "2022-07-01",
             "end_date": null, "description": null},
        ],
        "extra_knowledge": "Certificación en cocina",
    })
}

// ------------------------------------------------------------------ perfil

/// P1: una cuenta nueva tiene perfil vacío con la estructura completa, no 404.
#[tokio::test]
async fn p1_cuenta_nueva_devuelve_perfil_vacio() {
    let (db, app, token) = con_cuenta().await;
    let perfil = get_perfil(&app, &token).await;
    assert_eq!(
        perfil,
        json!({"description": null, "phone": null, "academic_level_id": null,
               "profession_id": null, "job_hierarchy_id": null,
               "skills": [], "languages": [], "experiences": [], "extra_knowledge": null})
    );
    db.cleanup().await;
}

/// P2: PUT guarda todo y GET lo devuelve igual (con las experiencias en el orden recibido).
#[tokio::test]
async fn p2_put_guarda_el_perfil_completo() {
    let (db, app, token) = con_cuenta().await;
    let cuerpo = perfil_completo(&db).await;
    let (estado, resp) = put(&app, &token, cuerpo.clone()).await;
    assert_eq!(estado, StatusCode::NO_CONTENT, "{resp}");

    let perfil = get_perfil(&app, &token).await;
    assert_eq!(perfil["description"], cuerpo["description"]);
    assert_eq!(perfil["academic_level_id"], cuerpo["academic_level_id"]);
    assert_eq!(perfil["profession_id"], cuerpo["profession_id"]);
    assert_eq!(perfil["job_hierarchy_id"], cuerpo["job_hierarchy_id"]);
    assert_eq!(perfil["extra_knowledge"], "Certificación en cocina");
    assert_eq!(perfil["phone"], TELEFONO);
    assert_eq!(perfil["skills"], cuerpo["skills"]);
    assert_eq!(perfil["languages"], cuerpo["languages"]);
    assert_eq!(perfil["experiences"], cuerpo["experiences"]);
    // `position` sigue el orden recibido.
    assert_eq!(
        db.count(
            "SELECT count(*) FROM applicant_experiences WHERE company = 'Acme' AND position = 0"
        )
        .await,
        1
    );
    assert_eq!(
        db.count(
            "SELECT count(*) FROM applicant_experiences WHERE company = 'Beta' AND position = 1"
        )
        .await,
        1
    );
    db.cleanup().await;
}

/// P3: el teléfono y la descripción de cada experiencia no están en claro en la base.
#[tokio::test]
async fn p3_telefono_y_descripcion_van_cifrados() {
    let (db, app, token) = con_cuenta().await;
    let (estado, _) = put(&app, &token, perfil_completo(&db).await).await;
    assert_eq!(estado, StatusCode::NO_CONTENT);

    let telefono = bytes_de(&db, "SELECT phone FROM personal_data").await;
    assert!(!contiene(&telefono, TELEFONO.as_bytes()));
    assert!(!contiene(&telefono, b"4567"));
    assert_eq!(pii().decrypt(&telefono).unwrap(), TELEFONO);

    let desc = bytes_de(
        &db,
        "SELECT description FROM applicant_experiences WHERE company = 'Acme'",
    )
    .await;
    assert!(!contiene(&desc, SECRETO.as_bytes()));
    assert_eq!(pii().decrypt(&desc).unwrap(), SECRETO);
    // Sin descripción no hay nada que cifrar.
    assert_eq!(
        db.count("SELECT count(*) FROM applicant_experiences WHERE description IS NULL")
            .await,
        1
    );
    db.cleanup().await;
}

/// P4: un teléfono con letras o de más de 20 caracteres → 422, y no cambia nada.
#[tokio::test]
async fn p4_telefono_invalido_da_422() {
    let (db, app, token) = con_cuenta().await;
    for malo in [
        "555-CALL-ME",
        "123456789012345678901",
        "12345<script>",
        "ext. 12",
    ] {
        let (estado, cuerpo) = put(&app, &token, json!({"phone": malo})).await;
        assert_eq!(estado, StatusCode::UNPROCESSABLE_ENTITY, "{malo}");
        assert_eq!(cuerpo["error"], "datos_invalidos");
        assert!(!cuerpo.to_string().contains(malo));
    }
    // 20 exactos sí valen.
    let (estado, _) = put(
        &app,
        &token,
        json!({"phone": "+52 55 1234-5678 (01)"[..20].to_string()}),
    )
    .await;
    assert_eq!(estado, StatusCode::NO_CONTENT);
    assert_eq!(db.count("SELECT count(*) FROM personal_data").await, 1);
    db.cleanup().await;
}

/// P5: un id de catálogo inexistente o inactivo → 422 y no se guarda nada.
#[tokio::test]
async fn p5_ids_de_catalogo_invalidos_dan_422() {
    let (db, app, token) = con_cuenta().await;
    let fantasma = "00000000-0000-7000-8000-000000000000";
    let bueno = skill(&db, 0).await;
    let casos = vec![
        json!({"skills": [{"skill_id": bueno, "level": null}, {"skill_id": fantasma}]}),
        json!({"languages": [{"language_id": fantasma, "proficiency": "basic"}]}),
        json!({"academic_level_id": fantasma}),
        json!({"profession_id": fantasma}),
        json!({"job_hierarchy_id": fantasma}),
        json!({"skills": [{"skill_id": "no-es-un-uuid"}]}),
    ];
    for caso in casos {
        let (estado, cuerpo) = put(&app, &token, caso.clone()).await;
        assert_eq!(estado, StatusCode::UNPROCESSABLE_ENTITY, "{caso}");
        assert_eq!(cuerpo["error"], "datos_invalidos");
    }
    // Un valor inactivo tampoco vale.
    db.exec(&format!(
        "UPDATE skills SET active = false WHERE id = '{bueno}'"
    ))
    .await
    .unwrap();
    let (estado, _) = put(&app, &token, json!({"skills": [{"skill_id": bueno}]})).await;
    assert_eq!(estado, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(db.count("SELECT count(*) FROM applicant_skills").await, 0);
    assert_eq!(db.count("SELECT count(*) FROM professional_data").await, 0);
    db.cleanup().await;
}

/// P6: PUT es completo (lo no enviado se vacía) e idempotente.
#[tokio::test]
async fn p6_put_es_completo_e_idempotente() {
    let (db, app, token) = con_cuenta().await;
    let cuerpo = perfil_completo(&db).await;
    put(&app, &token, cuerpo.clone()).await;
    let primero = get_perfil(&app, &token).await;
    let (estado, _) = put(&app, &token, cuerpo).await;
    assert_eq!(estado, StatusCode::NO_CONTENT);
    assert_eq!(get_perfil(&app, &token).await, primero, "idempotente");
    assert_eq!(db.count("SELECT count(*) FROM applicant_skills").await, 2);
    assert_eq!(
        db.count("SELECT count(*) FROM applicant_experiences").await,
        2
    );

    // Un PUT casi vacío lo vacía todo.
    let (estado, _) = put(&app, &token, json!({"description": "solo esto"})).await;
    assert_eq!(estado, StatusCode::NO_CONTENT);
    let perfil = get_perfil(&app, &token).await;
    assert_eq!(
        perfil,
        json!({"description": "solo esto", "phone": null, "academic_level_id": null,
               "profession_id": null, "job_hierarchy_id": null,
               "skills": [], "languages": [], "experiences": [], "extra_knowledge": null})
    );
    assert_eq!(
        db.count("SELECT count(*) FROM applicant_extra_knowledge")
            .await,
        0
    );
    assert_eq!(
        db.count("SELECT count(*) FROM personal_data WHERE phone IS NOT NULL")
            .await,
        0
    );
    db.cleanup().await;
}

/// P7: `/me` devuelve los campos de la Fase 1 más `roles` y `permissions` (Fase 5, paso 2): el
/// perfil no se cuela.
#[tokio::test]
async fn p7_me_no_cambia() {
    let (db, app, token) = con_cuenta().await;
    put(&app, &token, perfil_completo(&db).await).await;
    let (estado, me) = enviar(&app, "GET", "/api/v1/me", Some(&token), None).await;
    assert_eq!(estado, StatusCode::OK);
    let mut campos: Vec<&str> = me.as_object().unwrap().keys().map(String::as_str).collect();
    campos.sort();
    assert_eq!(
        campos,
        [
            "email",
            "first_name",
            "id",
            "last_name",
            "permissions",
            "plan",
            "roles",
            "status"
        ]
    );
    db.cleanup().await;
}

/// P8: no hay ruta por id de perfil y lo de una persona no toca lo de otra.
#[tokio::test]
async fn p8_el_perfil_de_otro_no_se_lee_ni_se_modifica() {
    let (db, app, a) = con_cuenta().await;
    let b = registrar(&app, "luis@correo-prueba.com").await;
    put(&app, &a, perfil_completo(&db).await).await;

    // B no ve nada de A y A no es alcanzable por id.
    let perfil_b = get_perfil(&app, &b).await;
    assert_eq!(perfil_b["phone"], Value::Null);
    assert_eq!(perfil_b["skills"], json!([]));
    let (user_a,): (String,) = (common::uuid_de(
        &db,
        "SELECT user_id FROM applicants ORDER BY created_at LIMIT 1",
    )
    .await
    .to_string(),);
    for metodo in ["GET", "PUT"] {
        let (estado, _) = enviar(
            &app,
            metodo,
            &format!("{PERFIL}/{user_a}"),
            Some(&b),
            Some(json!({})),
        )
        .await;
        assert_eq!(estado, StatusCode::NOT_FOUND);
    }
    // El PUT de B no modifica a A.
    put(&app, &b, json!({"description": "soy B"})).await;
    assert_eq!(
        get_perfil(&app, &a).await["description"],
        "Desarrolladora de software"
    );
    assert_eq!(get_perfil(&app, &a).await["phone"], TELEFONO);
    db.cleanup().await;
}

/// P9 / S2: sin token, 401 en todas las rutas nuevas.
#[tokio::test]
async fn p9_s2_sin_token_da_401() {
    let (db, app, _) = con_cuenta().await;
    let id = "00000000-0000-7000-8000-000000000000";
    let rutas = [
        ("GET", PERFIL.to_string()),
        ("PUT", PERFIL.to_string()),
        ("GET", format!("/api/v1/cvs/{id}/extraction")),
        ("POST", format!("/api/v1/cvs/{id}/extraction/confirm")),
    ];
    for (metodo, ruta) in rutas {
        for token in [None, Some("basura")] {
            let (estado, cuerpo) = enviar(&app, metodo, &ruta, token, Some(json!({}))).await;
            assert_eq!(estado, StatusCode::UNAUTHORIZED, "{metodo} {ruta}");
            assert_eq!(cuerpo["error"], "no_autenticado");
        }
    }
    db.cleanup().await;
}

// ------------------------------------------------- validaciones compartidas

/// Casos inválidos que valen igual para PUT y para confirm (V4–V9).
async fn casos_invalidos(db: &TestDb) -> Vec<(&'static str, Value)> {
    let s0 = skill(db, 0).await;
    let l0 = idioma(db, 0).await;
    let fantasma = "00000000-0000-7000-8000-000000000000";
    let largo = "x".repeat(2001);
    // Ids reales y distintos: así el único motivo de rechazo es el tope. Los idiomas sembrados
    // son pocos, así que se añaden los que faltan.
    db.exec(
        "INSERT INTO languages (id, code, name) \
         SELECT gen_random_uuid(), 'xl' || i, 'Extra ' || i FROM generate_series(1, 10) i",
    )
    .await
    .unwrap();
    let muchas_skills: Vec<Value> = ids(db, "skills", 61)
        .await
        .into_iter()
        .map(|id| json!({"skill_id": id}))
        .collect();
    let muchos_idiomas: Vec<Value> = ids(db, "languages", 11)
        .await
        .into_iter()
        .map(|id| json!({"language_id": id, "proficiency": "basic"}))
        .collect();
    let muchas_exp: Vec<Value> = (0..31).map(|i| experiencia(&format!("E{i}"))).collect();
    vec![
        (
            "V4 skill inexistente",
            json!({"skills": [{"skill_id": fantasma}]}),
        ),
        (
            "V5 skill repetida",
            json!({"skills": [{"skill_id": s0}, {"skill_id": s0.to_uppercase()}]}),
        ),
        (
            "V5b idioma repetido",
            json!({"languages": [
            {"language_id": l0, "proficiency": "basic"}, {"language_id": l0, "proficiency": "native"}]}),
        ),
        (
            "V6 proficiency fuera del enum",
            json!({"languages": [{"language_id": l0, "proficiency": "fluent"}]}),
        ),
        (
            "V6b nivel de skill fuera del enum",
            json!({"skills": [{"skill_id": s0, "level": "dios"}]}),
        ),
        (
            "V6c años negativos",
            json!({"skills": [{"skill_id": s0, "years_of_experience": -1}]}),
        ),
        (
            "V7 fin antes del inicio",
            json!({"experiences": [
            {"company": "A", "title": "B", "start_date": "2022-05-01", "end_date": "2022-04-30"}]}),
        ),
        (
            "V7b fecha imposible",
            json!({"experiences": [
            {"company": "A", "title": "B", "start_date": "2022-02-30"}]}),
        ),
        (
            "V7c empresa vacía",
            json!({"experiences": [
            {"company": "  ", "title": "B", "start_date": "2022-01-01"}]}),
        ),
        ("V8 descripción larga", json!({"description": largo})),
        (
            "V8b descripción de experiencia larga",
            json!({"experiences": [
            {"company": "A", "title": "B", "start_date": "2022-01-01", "description": largo}]}),
        ),
        ("V9 más de 60 skills", json!({"skills": muchas_skills})),
        (
            "V9b más de 10 idiomas",
            json!({"languages": muchos_idiomas}),
        ),
        (
            "V9c más de 30 experiencias",
            json!({"experiences": muchas_exp}),
        ),
        (
            "cuerpo que no es el formato",
            json!({"skills": "no es una lista"}),
        ),
    ]
}

/// V4–V9 para PUT: cada caso → 422 `datos_invalidos`, sin guardar nada.
#[tokio::test]
async fn put_rechaza_los_casos_invalidos() {
    let (db, app, token) = con_cuenta().await;
    for (nombre, cuerpo) in casos_invalidos(&db).await {
        let (estado, resp) = put(&app, &token, cuerpo).await;
        assert_eq!(estado, StatusCode::UNPROCESSABLE_ENTITY, "{nombre}: {resp}");
        assert_eq!(resp["error"], "datos_invalidos", "{nombre}");
    }
    for tabla in [
        "applicant_skills",
        "applicant_languages",
        "applicant_experiences",
        "professional_data",
        "personal_data",
    ] {
        assert_eq!(
            db.count(&format!("SELECT count(*) FROM {tabla}")).await,
            0,
            "{tabla}"
        );
    }
    db.cleanup().await;
}

/// V8: HTML y `<script>` se guardan como texto y se devuelven idénticos, nunca interpretados.
#[tokio::test]
async fn v8_el_html_se_guarda_como_texto() {
    let (db, app, token) = con_cuenta().await;
    let html = "<script>alert('x')</script><b>hola</b> & más";
    let (estado, _) = put(
        &app,
        &token,
        json!({"description": html, "experiences": [
            {"company": "<i>A</i>", "title": "B", "start_date": "2022-01-01", "description": html}]}),
    )
    .await;
    assert_eq!(estado, StatusCode::NO_CONTENT);
    let perfil = get_perfil(&app, &token).await;
    assert_eq!(perfil["description"], html);
    assert_eq!(perfil["experiences"][0]["description"], html);
    assert_eq!(perfil["experiences"][0]["company"], "<i>A</i>");
    // Exactamente 2000 caracteres sí caben.
    let (estado, _) = put(&app, &token, json!({"description": "é".repeat(2000)})).await;
    assert_eq!(estado, StatusCode::NO_CONTENT);
    db.cleanup().await;
}

// -------------------------------------------------------------- extracción

fn propuesta() -> Value {
    json!({"description": "Propuesta del extractor",
           "skills": [], "languages": [],
           "experiences": [{"company": "Propuesta SA", "title": "Dev", "start_date": "2020-01-01",
                            "end_date": null, "description": null, "confidence": "alta"}]})
}

/// Inserta un CV y su extracción directamente por SQL. Devuelve el id del CV.
async fn insertar_cv(db: &TestDb, user_id: &str, estado: &str, extraido: &Value) -> String {
    let cv = migration::new_id().to_string();
    let llave = pii().encrypt(&migration::new_id().to_string()).unwrap();
    db.exec(&format!(
        "INSERT INTO cvs (id, applicant_id, storage_key, original_filename, mime_type, size_bytes, \
                          version, is_primary) \
         SELECT '{cv}', id, '\\x{}'::bytea, 'cv.pdf', 'application/pdf', 10, 1, true \
         FROM applicants WHERE user_id = '{user_id}'",
        common::hex(&llave)
    ))
    .await
    .unwrap();
    let motivo = if estado == "failed" {
        "'sin_texto'"
    } else {
        "NULL"
    };
    db.exec(&format!(
        "INSERT INTO cv_extractions (id, cv_id, extracted, status, reason, model) \
         VALUES ('{}', '{cv}', '{}'::jsonb, '{estado}', {motivo}, 'deterministic')",
        migration::new_id(),
        extraido.to_string().replace('\'', "''")
    ))
    .await
    .unwrap();
    cv
}

async fn user_id(app: &Router, token: &str) -> String {
    let (_, me) = enviar(app, "GET", "/api/v1/me", Some(token), None).await;
    me["id"].as_str().unwrap().to_string()
}

fn extraction(cv: &str) -> String {
    format!("/api/v1/cvs/{cv}/extraction")
}

fn confirm(cv: &str) -> String {
    format!("/api/v1/cvs/{cv}/extraction/confirm")
}

/// V1: la extracción propia se devuelve con su confianza y `reviewed`; la de otro → 404 (S1).
#[tokio::test]
async fn v1_s1_la_extraccion_es_solo_del_dueno() {
    let (db, app, a) = con_cuenta().await;
    let b = registrar(&app, "luis@correo-prueba.com").await;
    let cv = insertar_cv(&db, &user_id(&app, &a).await, "ready", &propuesta()).await;

    let (estado, cuerpo) = enviar(&app, "GET", &extraction(&cv), Some(&a), None).await;
    assert_eq!(estado, StatusCode::OK, "{cuerpo}");
    assert_eq!(cuerpo["status"], "ready");
    assert_eq!(cuerpo["reason"], Value::Null);
    assert_eq!(cuerpo["reviewed"], false);
    assert_eq!(cuerpo["proposal"], propuesta());
    assert_eq!(cuerpo["proposal"]["experiences"][0]["confidence"], "alta");

    // S1: el usuario B pide el CV de A, uno inexistente y un id malformado → 404 idéntico.
    let ajeno = enviar(&app, "GET", &extraction(&cv), Some(&b), None).await;
    let fantasma = enviar(
        &app,
        "GET",
        &extraction("00000000-0000-7000-8000-000000000000"),
        Some(&b),
        None,
    )
    .await;
    let malo = enviar(&app, "GET", &extraction("no-es-uuid"), Some(&b), None).await;
    for (estado, cuerpo) in [&ajeno, &fantasma, &malo] {
        assert_eq!(*estado, StatusCode::NOT_FOUND);
        assert_eq!(cuerpo["error"], "no_encontrado");
    }
    assert_eq!(ajeno.1, fantasma.1);
    assert!(!ajeno.1.to_string().contains("Propuesta"));

    // B tampoco puede confirmar el CV de A, ni se toca el perfil de A ni de B.
    let (estado, _) = enviar(
        &app,
        "POST",
        &confirm(&cv),
        Some(&b),
        Some(json!({"description": "robo"})),
    )
    .await;
    assert_eq!(estado, StatusCode::NOT_FOUND);
    let (_, ext) = enviar(&app, "GET", &extraction(&cv), Some(&a), None).await;
    assert_eq!(ext["reviewed"], false);
    assert_eq!(db.count("SELECT count(*) FROM professional_data").await, 0);
    db.cleanup().await;
}

/// Una extracción `failed` se devuelve con su motivo y sin propuesta.
#[tokio::test]
async fn extraccion_fallida_trae_motivo_y_sin_propuesta() {
    let (db, app, a) = con_cuenta().await;
    let cv = insertar_cv(&db, &user_id(&app, &a).await, "failed", &json!({})).await;
    let (estado, cuerpo) = enviar(&app, "GET", &extraction(&cv), Some(&a), None).await;
    assert_eq!(estado, StatusCode::OK);
    assert_eq!(cuerpo["status"], "failed");
    assert_eq!(cuerpo["reason"], "sin_texto");
    assert_eq!(cuerpo["proposal"], Value::Null);
    db.cleanup().await;
}

/// V2 + V3 + V10: confirmar guarda lo editado (no lo propuesto), reemplaza lo previo y marca la
/// extracción como revisada.
#[tokio::test]
async fn v2_v3_v10_confirmar_guarda_lo_editado_y_reemplaza() {
    let (db, app, a) = con_cuenta().await;
    // Perfil previo, con teléfono y datos que el confirm no debe tocar salvo lo que reemplaza.
    put(&app, &a, perfil_completo(&db).await).await;
    let cv = insertar_cv(&db, &user_id(&app, &a).await, "ready", &propuesta()).await;

    let editado = json!({
        "description": "Lo que la persona escribió",
        "skills": [{"skill_id": skill(&db, 5).await, "level": "expert", "years_of_experience": 7}],
        "languages": [{"language_id": idioma(&db, 1).await, "proficiency": "advanced"}],
        "experiences": [{"company": "Editada SA", "title": "CTO", "start_date": "2019-03-01",
                         "end_date": null, "description": "mi texto"}],
    });
    let (estado, cuerpo) =
        enviar(&app, "POST", &confirm(&cv), Some(&a), Some(editado.clone())).await;
    assert_eq!(estado, StatusCode::NO_CONTENT, "{cuerpo}");

    let perfil = get_perfil(&app, &a).await;
    assert_eq!(perfil["description"], "Lo que la persona escribió");
    assert_eq!(perfil["skills"], editado["skills"]);
    assert_eq!(perfil["languages"], editado["languages"]);
    assert_eq!(perfil["experiences"], editado["experiences"]);
    assert!(
        !perfil.to_string().contains("Propuesta"),
        "se guardó lo propuesto"
    );
    // Lo ajeno al confirm sigue ahí.
    assert_eq!(perfil["phone"], TELEFONO);
    assert_eq!(perfil["extra_knowledge"], "Certificación en cocina");
    assert!(perfil["academic_level_id"].is_string());

    // V10
    assert_eq!(
        db.count("SELECT count(*) FROM cv_extractions WHERE reviewed_by_applicant")
            .await,
        1
    );
    let (_, ext) = enviar(&app, "GET", &extraction(&cv), Some(&a), None).await;
    assert_eq!(ext["reviewed"], true);
    // La descripción de la experiencia sigue cifrada.
    let desc = bytes_de(&db, "SELECT description FROM applicant_experiences").await;
    assert!(!contiene(&desc, b"mi texto"));
    db.cleanup().await;
}

/// V4–V9 en confirm: cada caso inválido → 422, no queda nada a medias y no se marca revisada.
#[tokio::test]
async fn v3_v4_v9_confirm_valida_igual_y_es_todo_o_nada() {
    let (db, app, a) = con_cuenta().await;
    put(&app, &a, perfil_completo(&db).await).await;
    let antes = get_perfil(&app, &a).await;
    let cv = insertar_cv(&db, &user_id(&app, &a).await, "ready", &propuesta()).await;

    for (nombre, mut cuerpo) in casos_invalidos(&db).await {
        // Un tramo válido por delante: si algo se guardara a medias, se vería.
        if cuerpo.get("description").is_none() {
            cuerpo["description"] = json!("descripción que no debe quedar");
        }
        let (estado, resp) = enviar(&app, "POST", &confirm(&cv), Some(&a), Some(cuerpo)).await;
        assert_eq!(estado, StatusCode::UNPROCESSABLE_ENTITY, "{nombre}: {resp}");
        assert_eq!(resp["error"], "datos_invalidos", "{nombre}");
        assert_eq!(
            get_perfil(&app, &a).await,
            antes,
            "{nombre}: quedó algo a medias"
        );
    }
    assert_eq!(
        db.count("SELECT count(*) FROM cv_extractions WHERE reviewed_by_applicant")
            .await,
        0
    );
    db.cleanup().await;
}

/// V3 (todo o nada de verdad): un fallo del último INSERT revierte lo anterior.
#[tokio::test]
async fn v3_un_fallo_a_mitad_revierte_todo() {
    let (db, app, a) = con_cuenta().await;
    put(&app, &a, perfil_completo(&db).await).await;
    let antes = get_perfil(&app, &a).await;
    let cv = insertar_cv(&db, &user_id(&app, &a).await, "ready", &propuesta()).await;
    // La extracción desaparece entre medias de la validación y el guardado: simulado quitando
    // la fila y dejando el CV; el UPDATE final no marca nada y la transacción entera se deshace.
    db.exec("DELETE FROM cv_extractions").await.unwrap();
    let cuerpo = json!({"description": "nueva", "skills": [{"skill_id": skill(&db, 9).await}]});
    let (estado, _) = enviar(&app, "POST", &confirm(&cv), Some(&a), Some(cuerpo)).await;
    assert_eq!(estado, StatusCode::NOT_FOUND);
    assert_eq!(get_perfil(&app, &a).await, antes);
    db.cleanup().await;
}

/// V11: confirmar tal cual lo propuesto (la persona acepta sin editar) guarda lo propuesto.
#[tokio::test]
async fn v11_confirmar_lo_propuesto_tal_cual() {
    let (db, app, a) = con_cuenta().await;
    let s = skill(&db, 2).await;
    let l = idioma(&db, 2).await;
    let mut prop = propuesta();
    prop["skills"] = json!([{"skill_id": s, "code": "x", "name": "X", "confidence": "media"}]);
    prop["languages"] = json!([{"language_id": l, "code": "y", "name": "Y", "proficiency": "basic",
                                "to_confirm": true, "confidence": "baja"}]);
    let cv = insertar_cv(&db, &user_id(&app, &a).await, "ready", &prop).await;

    let (_, ext) = enviar(&app, "GET", &extraction(&cv), Some(&a), None).await;
    let p = &ext["proposal"];
    let aceptado = json!({
        "description": p["description"],
        "skills": [{"skill_id": p["skills"][0]["skill_id"], "level": null, "years_of_experience": null}],
        "languages": [{"language_id": p["languages"][0]["language_id"], "proficiency": p["languages"][0]["proficiency"]}],
        "experiences": [{"company": p["experiences"][0]["company"], "title": p["experiences"][0]["title"],
                         "start_date": p["experiences"][0]["start_date"], "end_date": p["experiences"][0]["end_date"],
                         "description": p["experiences"][0]["description"]}],
    });
    let (estado, cuerpo) = enviar(
        &app,
        "POST",
        &confirm(&cv),
        Some(&a),
        Some(aceptado.clone()),
    )
    .await;
    assert_eq!(estado, StatusCode::NO_CONTENT, "{cuerpo}");
    let perfil = get_perfil(&app, &a).await;
    assert_eq!(perfil["description"], "Propuesta del extractor");
    assert_eq!(perfil["skills"], aceptado["skills"]);
    assert_eq!(perfil["languages"], aceptado["languages"]);
    assert_eq!(perfil["experiences"], aceptado["experiences"]);
    db.cleanup().await;
}

/// V12: una extracción `failed` se confirma igual, con datos escritos a mano.
#[tokio::test]
async fn v12_confirmar_una_extraccion_fallida() {
    let (db, app, a) = con_cuenta().await;
    let cv = insertar_cv(&db, &user_id(&app, &a).await, "failed", &json!({})).await;
    let cuerpo = json!({"description": "A mano", "skills": [{"skill_id": skill(&db, 3).await}],
                        "experiences": [experiencia("Manual SA")]});
    let (estado, resp) = enviar(&app, "POST", &confirm(&cv), Some(&a), Some(cuerpo)).await;
    assert_eq!(estado, StatusCode::NO_CONTENT, "{resp}");
    let perfil = get_perfil(&app, &a).await;
    assert_eq!(perfil["description"], "A mano");
    assert_eq!(perfil["experiences"][0]["company"], "Manual SA");
    assert_eq!(
        db.count("SELECT count(*) FROM cv_extractions WHERE reviewed_by_applicant")
            .await,
        1
    );
    db.cleanup().await;
}

/// El confirm no guarda un `phone` aunque lo envíen (no es parte de su formato).
#[tokio::test]
async fn confirm_ignora_el_telefono() {
    let (db, app, a) = con_cuenta().await;
    let cv = insertar_cv(&db, &user_id(&app, &a).await, "ready", &propuesta()).await;
    let (estado, _) = enviar(
        &app,
        "POST",
        &confirm(&cv),
        Some(&a),
        Some(json!({"phone": "123456"})),
    )
    .await;
    assert_eq!(estado, StatusCode::NO_CONTENT);
    assert_eq!(get_perfil(&app, &a).await["phone"], Value::Null);
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
            // El mismo filtro que `main.rs`. Con `RUST_LOG=sea_orm=debug` el span de sea-orm
            // imprime los valores de cada consulta (descripciones, empresas): no se debe usar
            // en producción, y esta prueba no lo ejercita.
            .with_env_filter("jobout_core_api=debug,tower_http=debug")
            .with_ansi(false)
            .with_writer(move || escritor.clone())
            .finish(),
    );
    // Marca propia: se emite con el suscriptor de ESTA prueba ya activo, así que su recepción no
    // depende de que `tracing` entregue eventos del servicio (que puede perderlos en una carrera
    // entre pruebas que instalan suscriptores a la vez). Es la comprobación de cordura.
    tracing::info!(target: "jobout_core_api::test", "captura-activa");
    (registro, guarda)
}

/// S3: un error provocado con datos sensibles no los filtra ni en la respuesta ni en el log.
#[tokio::test]
async fn s3_los_errores_no_filtran_datos_personales() {
    let (log, _guarda) = capturar_log();
    let (db, app, a) = con_cuenta().await;
    let cv = insertar_cv(&db, &user_id(&app, &a).await, "ready", &propuesta()).await;
    let s = skill(&db, 0).await;
    let sensibles = [
        "secreto-telefono-77",
        "secreto-empresa-88",
        "secreto-texto-99",
        "secreto-fecha-11",
    ];

    let casos = vec![
        ("PUT", PERFIL.to_string(), json!({"phone": sensibles[0]})),
        (
            "PUT",
            PERFIL.to_string(),
            json!({"skills": [{"skill_id": s}, {"skill_id": s}],
            "experiences": [{"company": sensibles[1], "title": sensibles[1], "start_date": "2022-01-01",
                             "description": sensibles[2]}]}),
        ),
        (
            "PUT",
            PERFIL.to_string(),
            json!({"experiences": [{"company": sensibles[1], "title": "t",
            "start_date": sensibles[3], "description": sensibles[2]}]}),
        ),
        ("PUT", PERFIL.to_string(), json!({"skills": sensibles[2]})),
        (
            "POST",
            confirm(&cv),
            json!({"description": "x".repeat(2001), "experiences": [
            {"company": sensibles[1], "title": "t", "start_date": "2022-01-01", "description": sensibles[2]}]}),
        ),
        (
            "POST",
            confirm(&cv),
            json!({"languages": [{"language_id": sensibles[0], "proficiency": sensibles[2]}]}),
        ),
    ];
    for (metodo, ruta, cuerpo) in casos {
        let (estado, resp) = enviar(&app, metodo, &ruta, Some(&a), Some(cuerpo)).await;
        assert_eq!(estado, StatusCode::UNPROCESSABLE_ENTITY, "{resp}");
        for s in sensibles {
            assert!(!resp.to_string().contains(s), "la respuesta filtra {s}");
        }
    }
    // JSON roto con un secreto dentro.
    let (estado, resp) = enviar_crudo(
        &app,
        "PUT",
        PERFIL,
        Some(&a),
        Body::from(format!("{{\"phone\": \"{}\", ", sensibles[0])),
    )
    .await;
    assert_eq!(estado, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(resp["error"], "datos_invalidos");

    let texto = log.texto();
    assert!(texto.contains("captura-activa"), "no se capturó ningún log");
    for s in sensibles {
        assert!(!texto.contains(s), "el log filtra {s}");
    }
    db.cleanup().await;
}

/// S4: el log de peticiones correctas no contiene teléfono ni texto del perfil.
#[tokio::test]
async fn s4_el_log_no_contiene_datos_personales() {
    let (log, _guarda) = capturar_log();
    let (db, app, a) = con_cuenta().await;
    let (estado, _) = put(&app, &a, perfil_completo(&db).await).await;
    assert_eq!(estado, StatusCode::NO_CONTENT);
    get_perfil(&app, &a).await;
    let cv = insertar_cv(&db, &user_id(&app, &a).await, "ready", &propuesta()).await;
    enviar(&app, "GET", &extraction(&cv), Some(&a), None).await;
    let (estado, _) = enviar(
        &app,
        "POST",
        &confirm(&cv),
        Some(&a),
        Some(json!({"description": "descripcion-secreta-55", "experiences": [
            {"company": "empresa-secreta-66", "title": "t", "start_date": "2022-01-01", "description": SECRETO}]})),
    )
    .await;
    assert_eq!(estado, StatusCode::NO_CONTENT);

    let texto = log.texto();
    assert!(texto.contains("captura-activa"), "no se capturó ningún log");
    for s in [
        TELEFONO,
        "4567",
        SECRETO,
        "descripcion-secreta-55",
        "empresa-secreta-66",
        "Desarrolladora",
    ] {
        assert!(!texto.contains(s), "el log contiene {s}");
    }
    db.cleanup().await;
}

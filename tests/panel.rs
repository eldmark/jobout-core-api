//! Casos de verdad del panel del reclutador (Fase 5a, paso 7, HU-52 a HU-54: PA1–PA10, SX2–SX6,
//! BO1 en lo de favoritos), contra «Panel del reclutador» de `api.md`. PostgreSQL real, una base
//! por prueba. Se escribieron antes que el código, en rojo.
//!
//! Empresas, ofertas y perfiles se insertan por SQL; las aplicaciones, por la ruta de aplicar.

mod common;
#[path = "common/empresas.rs"]
mod empresas;

use axum::http::StatusCode;
use empresas::{capturar_log, mundo, pii, Mundo, Persona, Resp, CLAVE};
use sea_orm::ConnectionTrait;
use serde_json::{json, Value};

const ID_NO: &str = "00000000-0000-7000-8000-000000000000";
const MINIMO: [&str; 3] = ["display_name", "skills", "experience"];

// ------------------------------------------------------------------ fixtures

async fn empresa_sql(m: &Mundo, nombre: &str) -> String {
    let id = m.texto("SELECT gen_random_uuid()::text").await;
    m.db.exec(&format!(
        "INSERT INTO companies (id, name, tax_id, tax_id_hash, website, country_id, employment_sector_id, verified_at) \
         VALUES ('{id}', '{nombre}', '\\x01'::bytea, '\\x{}'::bytea, 'https://{nombre}.example', \
                 (SELECT id FROM countries ORDER BY code LIMIT 1), \
                 (SELECT id FROM employment_sectors ORDER BY code LIMIT 1), now())",
        common::hex(nombre.as_bytes()),
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "INSERT INTO memberships (id, company_id, membership_type_id, start_date, expiration_date, status) \
         VALUES (gen_random_uuid(), '{id}', (SELECT id FROM membership_types ORDER BY code LIMIT 1), \
                 current_date - 10, current_date + 100, 'active')"
    ))
    .await
    .unwrap();
    id
}

async fn oferta_sql(m: &Mundo, company: &str, titulo: &str, confidencial: bool) -> String {
    let id = m.texto("SELECT gen_random_uuid()::text").await;
    m.db.exec(&format!(
        "INSERT INTO job_offers (id, company_id, name, description, status, is_confidential, publication_date) \
         VALUES ('{id}', '{company}', '{titulo}', 'Una oferta.', 'published', {confidencial}, current_date)"
    ))
    .await
    .unwrap();
    id
}

async fn reclutador_sql(m: &Mundo, tag: &str, company: &str) -> Persona {
    let p = m.persona(tag).await;
    m.dar_rol(&p.id, "recruiter").await;
    m.db.exec(&format!(
        "INSERT INTO company_recruiters (id, user_id, company_id, role) \
         VALUES (gen_random_uuid(), '{}', '{company}', 'recruiter')",
        p.id
    ))
    .await
    .unwrap();
    p
}

const SECRETO_TEL: &str = "+502 5555 9999";
const SECRETO_PERFIL: &str = "SECRETO-PERFIL-PROFESIONAL";
const SECRETO_NOTA: &str = "NOTA-INTERNA-SECRETA-777";

async fn perfil_sql(m: &Mundo, p: &Persona) {
    let a = m
        .texto(&format!(
            "SELECT id::text FROM applicants WHERE user_id = '{}'",
            p.id
        ))
        .await;
    let llaves = pii();
    m.db.exec(&format!(
        "INSERT INTO professional_data (id, applicant_id, description) \
         VALUES (gen_random_uuid(), '{a}', '{SECRETO_PERFIL}')"
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "INSERT INTO applicant_skills (id, applicant_id, skill_id, level, years_of_experience) \
         VALUES (gen_random_uuid(), '{a}', (SELECT id FROM skills ORDER BY code LIMIT 1), 'advanced', 3)"
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "INSERT INTO applicant_experiences (id, applicant_id, company, title, start_date, position) \
         VALUES (gen_random_uuid(), '{a}', 'Banco X', 'Analista', '2022-01-01', 0)"
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "INSERT INTO personal_data (id, applicant_id, phone) VALUES (gen_random_uuid(), '{a}', '\\x{}'::bytea)",
        common::hex(&llaves.encrypt(SECRETO_TEL).unwrap())
    ))
    .await
    .unwrap();
}

struct Esc {
    m: Mundo,
    ana: Persona,
    beto: Persona,
    rec: Persona,
    emp: String,
    oferta: String,
    /// Otra empresa, con su reclutador y su oferta.
    rec2: Persona,
    emp2: String,
    oferta2: String,
}

async fn escenario() -> Esc {
    let m = mundo().await;
    let emp = empresa_sql(&m, "cafenuevo").await;
    let oferta = oferta_sql(&m, &emp, "Barista", false).await;
    let emp2 = empresa_sql(&m, "otraempresa").await;
    let oferta2 = oferta_sql(&m, &emp2, "Cajero", false).await;
    let ana = m.persona("ana").await;
    let beto = m.persona("beto").await;
    perfil_sql(&m, &ana).await;
    perfil_sql(&m, &beto).await;
    let rec = reclutador_sql(&m, "rec", &emp).await;
    let rec2 = reclutador_sql(&m, "rec2", &emp2).await;
    Esc {
        m,
        ana,
        beto,
        rec,
        emp,
        oferta,
        rec2,
        emp2,
        oferta2,
    }
}

async fn aplicar(e: &Esc, p: &Persona, oferta: &str, campos: &[&str]) -> String {
    let r =
        e.m.enviar(
            "POST",
            &format!("/api/v1/offers/{oferta}/applications"),
            Some(&p.token),
            Some(json!({"consent": true, "shared_fields": campos})),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "aplicar: {}", r.text);
    r.json["id"].as_str().unwrap().to_string()
}

fn lista_de(oferta: &str) -> String {
    format!("/api/v1/company/offers/{oferta}/applications")
}

fn detalle_de(app: &str) -> String {
    format!("/api/v1/company/applications/{app}")
}

async fn lista(e: &Esc, p: &Persona, oferta: &str, q: &str) -> Resp {
    let sep = if q.is_empty() { "" } else { "?" };
    e.m.get(&format!("{}{sep}{q}", lista_de(oferta)), p).await
}

async fn mover(e: &Esc, p: &Persona, app: &str, cuerpo: Value) -> Resp {
    e.m.enviar(
        "POST",
        &format!("{}/status", detalle_de(app)),
        Some(&p.token),
        Some(cuerpo),
    )
    .await
}

async fn favorito(e: &Esc, p: &Persona, metodo: &str, app: &str) -> Resp {
    e.m.enviar(
        metodo,
        &format!("{}/favorite", detalle_de(app)),
        Some(&p.token),
        None,
    )
    .await
}

fn codigo(r: &Resp) -> &str {
    r.json["error"].as_str().unwrap_or("")
}

fn ids(r: &Resp) -> Vec<String> {
    r.json["items"]
        .as_array()
        .unwrap_or_else(|| panic!("sin items: {}", r.text))
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect()
}

fn claves(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

async fn retirar(e: &Esc, p: &Persona, app: &str) {
    let r =
        e.m.enviar(
            "POST",
            &format!("/api/v1/me/applications/{app}/withdraw"),
            Some(&p.token),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "retirar: {}", r.text);
}

// ------------------------------------------------------------------ PA1, SX2, SX6

/// PA1: la lista es de **mi** oferta; la de otra empresa, una inexistente y una mal formada dan
/// el mismo 404 (SX2, SX6).
#[tokio::test]
async fn pa1_la_lista_es_solo_de_mi_oferta() {
    let e = escenario().await;
    let a = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    let b = aplicar(&e, &e.beto, &e.oferta, &MINIMO).await;
    // Ana también aplica a la oferta de la otra empresa: no aparece en la mía dos veces.
    aplicar(&e, &e.ana, &e.oferta2, &MINIMO).await;

    let r = lista(&e, &e.rec, &e.oferta, "").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let mut v = ids(&r);
    v.sort();
    let mut esperado = vec![a, b];
    esperado.sort();
    assert_eq!(v, esperado);
    assert!(r.json["next_cursor"].is_null());

    let ajena = lista(&e, &e.rec, &e.oferta2, "").await;
    let inexistente = lista(&e, &e.rec, ID_NO, "").await;
    let mal = lista(&e, &e.rec, "no-es-uuid", "").await;
    let desde_la_otra = lista(&e, &e.rec2, &e.oferta, "").await;
    for r in [&ajena, &inexistente, &mal, &desde_la_otra] {
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.text);
        assert_eq!(r.text, ajena.text, "el 404 no revela si existe");
    }
    e.m.db.cleanup().await;
}

/// SX2/SX6: el detalle, el cambio de estado y los favoritos de una aplicación de otra empresa,
/// inexistente o mal formada dan el mismo 404, y no tocan nada.
#[tokio::test]
async fn la_aplicacion_ajena_es_404_en_todas_las_rutas() {
    let e = escenario().await;
    let a = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    for id in [a.as_str(), ID_NO, "no-es-uuid"] {
        let r = e.m.get(&detalle_de(id), &e.rec2).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.text);
        let r = mover(&e, &e.rec2, id, json!({"to": "under_review"})).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.text);
        for metodo in ["PUT", "DELETE"] {
            let r = favorito(&e, &e.rec2, metodo, id).await;
            assert_eq!(r.status, StatusCode::NOT_FOUND, "{metodo}: {}", r.text);
        }
    }
    assert_eq!(
        e.m.n(&format!(
            "SELECT count(*) FROM job_applications WHERE id = '{a}' AND status = 'applied'"
        ))
        .await,
        1
    );
    assert_eq!(
        e.m.n("SELECT count(*) FROM company_favorite_applicants")
            .await,
        0
    );
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ PA2, PA3

/// PA2: la lista trae lo mínimo: `display_name` y nada del contacto ni del perfil.
#[tokio::test]
async fn pa2_la_lista_solo_trae_el_nombre_visible() {
    let e = escenario().await;
    aplicar(
        &e,
        &e.ana,
        &e.oferta,
        &[
            "display_name",
            "skills",
            "experience",
            "email",
            "phone",
            "profile_description",
        ],
    )
    .await;
    let r = lista(&e, &e.rec, &e.oferta, "").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let item = &r.json["items"][0];
    assert_eq!(
        claves(item),
        [
            "applicant",
            "applied_at",
            "availability",
            "id",
            "is_favorite",
            "shared_fields",
            "status",
            "status_changed_at"
        ]
    );
    assert_eq!(item["applicant"], json!({"display_name": "Ana P."}));
    assert_eq!(item["availability"], "available");
    assert_eq!(item["status"], "applied");
    assert_eq!(item["is_favorite"], false);
    for prohibido in [
        e.ana.email.as_str(),
        SECRETO_TEL,
        SECRETO_PERFIL,
        e.ana.id.as_str(),
        "Banco X",
    ] {
        assert!(!r.text.contains(prohibido), "la lista trae {prohibido:?}");
    }
    e.m.db.cleanup().await;
}

/// PA2/PA3: el detalle trae **solo** lo compartido, la oferta y el historial; nunca el id de la
/// persona, su CV ni lo que no marcó.
#[tokio::test]
async fn pa3_el_detalle_trae_solo_lo_compartido() {
    let e = escenario().await;
    let a = aplicar(
        &e,
        &e.ana,
        &e.oferta,
        &["display_name", "skills", "experience", "email"],
    )
    .await;
    let r = e.m.get(&detalle_de(&a), &e.rec).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let v = &r.json;
    assert_eq!(
        claves(v),
        [
            "applicant",
            "applied_at",
            "availability",
            "history",
            "id",
            "is_favorite",
            "offer",
            "shared_fields",
            "status",
            "status_changed_at"
        ]
    );
    assert_eq!(v["id"], a.as_str());
    assert_eq!(v["offer"], json!({"id": e.oferta, "title": "Barista"}));
    assert_eq!(
        claves(&v["applicant"]),
        ["display_name", "email", "experience", "skills"]
    );
    assert_eq!(v["applicant"]["email"], e.ana.email.as_str());
    assert_eq!(v["applicant"]["display_name"], "Ana P.");
    assert_eq!(
        v["shared_fields"],
        json!(["display_name", "skills", "experience", "email"])
    );
    let h = v["history"].as_array().unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(claves(&h[0]), ["at", "from", "note", "to"]);
    assert_eq!(h[0]["from"], Value::Null);
    assert_eq!(h[0]["to"], "applied");
    assert_eq!(h[0]["note"], Value::Null);
    let ap_id =
        e.m.texto(&format!(
            "SELECT id::text FROM applicants WHERE user_id = '{}'",
            e.ana.id
        ))
        .await;
    for prohibido in [
        SECRETO_TEL,
        SECRETO_PERFIL,
        e.ana.id.as_str(),
        ap_id.as_str(),
        "storage_key",
        "\"cv",
        "changed_by",
    ] {
        assert!(!r.text.contains(prohibido), "el detalle trae {prohibido:?}");
    }
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ PA4, PA5

/// PA4/PA5: la máquina de estados del reclutador por la ruta; los finales no se reabren y repetir
/// un estado es 409 con el estado vigente.
#[tokio::test]
async fn pa4_cambiar_de_estado_respeta_la_maquina() {
    let e = escenario().await;
    let a = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;

    let r = mover(&e, &e.rec, &a, json!({"to": "shortlisted"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text);
    assert_eq!(codigo(&r), "estado_invalido");
    assert_eq!(r.json["status"], "applied");

    let r = mover(&e, &e.rec, &a, json!({"to": "under_review"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json["status"], "under_review");
    assert_eq!(r.json["applicant"]["display_name"], "Ana P.");
    let h = r.json["history"].as_array().unwrap();
    assert_eq!(h.len(), 2);
    assert_eq!(h[1]["from"], "applied");
    assert_eq!(h[1]["to"], "under_review");

    let r = mover(&e, &e.rec, &a, json!({"to": "under_review"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text);
    assert_eq!(r.json["status"], "under_review");

    for paso in ["shortlisted", "hired"] {
        let r = mover(&e, &e.rec, &a, json!({"to": paso})).await;
        assert_eq!(r.status, StatusCode::OK, "{paso}: {}", r.text);
    }
    for fin in ["rejected", "shortlisted", "under_review"] {
        let r = mover(&e, &e.rec, &a, json!({"to": fin})).await;
        assert_eq!(r.status, StatusCode::CONFLICT, "{fin}: {}", r.text);
        assert_eq!(r.json["status"], "hired");
    }
    assert_eq!(
        e.m.n(&format!(
            "SELECT count(*) FROM job_application_events WHERE job_application_id = '{a}'"
        ))
        .await,
        4
    );
    e.m.db.cleanup().await;
}

/// Cuerpo cerrado: `to` desconocido, `withdrawn` o `applied`, una nota de más de 500 caracteres
/// → 422 `datos_invalidos`; un campo de más → `campo_no_permitido`. Todo antes del 404.
#[tokio::test]
async fn el_cuerpo_del_cambio_de_estado_es_cerrado() {
    let e = escenario().await;
    let a = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    for (cuerpo, error) in [
        (json!({"to": "foo"}), "datos_invalidos"),
        (json!({"to": "withdrawn"}), "datos_invalidos"),
        (json!({"to": "applied"}), "datos_invalidos"),
        (json!({}), "datos_invalidos"),
        (json!({"to": 3}), "datos_invalidos"),
        (
            json!({"to": "under_review", "note": "x".repeat(501)}),
            "datos_invalidos",
        ),
        (
            json!({"to": "under_review", "reason": "x"}),
            "campo_no_permitido",
        ),
    ] {
        for id in [a.as_str(), ID_NO] {
            let r = mover(&e, &e.rec, id, cuerpo.clone()).await;
            assert_eq!(
                r.status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{cuerpo}: {}",
                r.text
            );
            assert_eq!(codigo(&r), error, "{cuerpo}");
        }
    }
    // 500 caracteres exactos sí.
    let r = mover(
        &e,
        &e.rec,
        &a,
        json!({"to": "under_review", "note": "é".repeat(500)}),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    e.m.db.cleanup().await;
}

/// La nota es interna: se guarda cifrada, la ve el panel en `history` y nunca la persona.
#[tokio::test]
async fn la_nota_se_guarda_cifrada_y_solo_la_ve_el_panel() {
    let e = escenario().await;
    let a = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    let r = mover(
        &e,
        &e.rec,
        &a,
        json!({"to": "under_review", "note": SECRETO_NOTA}),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json["history"][1]["note"], SECRETO_NOTA);

    let crudo = common::bytes_de(
        &e.m.db,
        &format!(
            "SELECT note FROM job_application_events WHERE job_application_id = '{a}' AND note IS NOT NULL"
        ),
    )
    .await;
    assert!(!common::contiene(&crudo, SECRETO_NOTA.as_bytes()));
    assert_eq!(pii().decrypt(&crudo).unwrap(), SECRETO_NOTA);

    let mia =
        e.m.get(&format!("/api/v1/me/applications/{a}"), &e.ana)
            .await;
    assert_eq!(mia.status, StatusCode::OK);
    assert!(!mia.text.contains(SECRETO_NOTA));
    // Otra empresa no la ve: ni siquiera ve la aplicación.
    let otra = e.m.get(&detalle_de(&a), &e.rec2).await;
    assert!(!otra.text.contains(SECRETO_NOTA));
    e.m.db.cleanup().await;
}

/// HU-55: mover a `shortlisted` una oferta confidencial revela la empresa a esa persona.
#[tokio::test]
async fn preseleccionar_una_confidencial_revela_la_empresa() {
    let e = escenario().await;
    let conf = oferta_sql(&e.m, &e.emp, "Confidencial", true).await;
    let a = aplicar(&e, &e.ana, &conf, &MINIMO).await;
    let mia = format!("/api/v1/me/applications/{a}");
    let r = e.m.get(&mia, &e.ana).await;
    assert_eq!(r.json["offer"]["company"], Value::Null);
    for paso in ["under_review", "shortlisted"] {
        let r = mover(&e, &e.rec, &a, json!({"to": paso})).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    }
    let r = e.m.get(&mia, &e.ana).await;
    assert_eq!(r.json["offer"]["company"]["name"], "cafenuevo");
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ PA6, PA7

/// PA6/PA7: el favorito es uno por (empresa, persona), idempotente, vale para todas sus
/// aplicaciones en la empresa y no lo ve otra empresa.
#[tokio::test]
async fn pa6_pa7_favoritos_por_empresa_y_persona() {
    let e = escenario().await;
    let otra_mia = oferta_sql(&e.m, &e.emp, "Mesero", false).await;
    let a1 = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    let a2 = aplicar(&e, &e.ana, &otra_mia, &MINIMO).await;
    let b = aplicar(&e, &e.beto, &e.oferta, &MINIMO).await;
    let a_otra = aplicar(&e, &e.ana, &e.oferta2, &MINIMO).await;

    for _ in 0..2 {
        let r = favorito(&e, &e.rec, "PUT", &a1).await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    }
    assert_eq!(
        e.m.n("SELECT count(*) FROM company_favorite_applicants")
            .await,
        1
    );
    let r = e.m.get(&detalle_de(&a2), &e.rec).await;
    assert_eq!(
        r.json["is_favorite"], true,
        "vale para la persona en la empresa"
    );
    let r = lista(&e, &e.rec, &e.oferta, "favorite=true").await;
    assert_eq!(ids(&r), vec![a1.clone()]);
    let r = lista(&e, &e.rec, &e.oferta, "favorite=false").await;
    assert_eq!(ids(&r), vec![b.clone()]);
    // La otra empresa no ve el favorito.
    let r = e.m.get(&detalle_de(&a_otra), &e.rec2).await;
    assert_eq!(r.json["is_favorite"], false);
    let r = lista(&e, &e.rec2, &e.oferta2, "favorite=true").await;
    assert!(ids(&r).is_empty());

    for _ in 0..2 {
        let r = favorito(&e, &e.rec, "DELETE", &a2).await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    }
    assert_eq!(
        e.m.n("SELECT count(*) FROM company_favorite_applicants")
            .await,
        0
    );
    e.m.db.cleanup().await;
}

/// Sobre una aplicación que ya no está disponible (retirada), marcar favorito → 409.
#[tokio::test]
async fn favorito_sobre_una_no_disponible_es_409() {
    let e = escenario().await;
    let a = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    retirar(&e, &e.ana, &a).await;
    let r = favorito(&e, &e.rec, "PUT", &a).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text);
    assert_eq!(codigo(&r), "estado_invalido");
    assert_eq!(
        e.m.n("SELECT count(*) FROM company_favorite_applicants")
            .await,
        0
    );
    // Quitarlo sigue siendo idempotente.
    let r = favorito(&e, &e.rec, "DELETE", &a).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    e.m.db.cleanup().await;
}

/// Al retirar, el favorito se borra si a la persona no le queda otra aplicación viva en la empresa.
#[tokio::test]
async fn retirar_borra_el_favorito_si_no_queda_otra_viva() {
    let e = escenario().await;
    let otra_mia = oferta_sql(&e.m, &e.emp, "Mesero", false).await;
    let a1 = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    let a2 = aplicar(&e, &e.ana, &otra_mia, &MINIMO).await;
    assert_eq!(
        favorito(&e, &e.rec, "PUT", &a1).await.status,
        StatusCode::NO_CONTENT
    );
    retirar(&e, &e.ana, &a1).await;
    assert_eq!(
        e.m.n("SELECT count(*) FROM company_favorite_applicants")
            .await,
        1,
        "le queda a2 viva"
    );
    retirar(&e, &e.ana, &a2).await;
    assert_eq!(
        e.m.n("SELECT count(*) FROM company_favorite_applicants")
            .await,
        0
    );
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ PA8

async fn auditoria(e: &Esc) -> Vec<(String, String, String, String)> {
    let filas =
        e.m.db
            .conn
            .query_all(sea_orm::Statement::from_string(
                sea_orm::DbBackend::Postgres,
                "SELECT action, resource_type, resource_id::text, actor_user_id::text \
             FROM access_audit ORDER BY created_at, id"
                    .to_string(),
            ))
            .await
            .unwrap();
    filas
        .iter()
        .map(|f| {
            (
                f.try_get_by_index(0).unwrap(),
                f.try_get_by_index(1).unwrap(),
                f.try_get_by_index(2).unwrap(),
                f.try_get_by_index(3).unwrap(),
            )
        })
        .collect()
}

/// PA8: la lista deja **una** fila (`application.list` sobre la oferta); el detalle y el cambio
/// de estado, una cada uno sobre la aplicación. Sin el dato; la IP, hasheada.
#[tokio::test]
async fn pa8_cada_acceso_deja_su_fila_de_auditoria() {
    let e = escenario().await;
    let a = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    aplicar(&e, &e.beto, &e.oferta, &MINIMO).await;
    let r =
        e.m.enviar_crudo(
            "GET",
            &lista_de(&e.oferta),
            Some(&e.rec.token),
            Vec::new(),
            &[("x-forwarded-for", "203.0.113.9")],
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(
        e.m.get(&detalle_de(&a), &e.rec).await.status,
        StatusCode::OK
    );
    assert_eq!(
        mover(&e, &e.rec, &a, json!({"to": "under_review"}))
            .await
            .status,
        StatusCode::OK
    );
    // Favoritos no devuelven datos de la persona: no se auditan.
    assert_eq!(
        favorito(&e, &e.rec, "PUT", &a).await.status,
        StatusCode::NO_CONTENT
    );
    let rec = e.rec.id.clone();
    assert_eq!(
        auditoria(&e).await,
        vec![
            (
                "application.list".into(),
                "job_offer".into(),
                e.oferta.clone(),
                rec.clone()
            ),
            (
                "application.view".into(),
                "job_application".into(),
                a.clone(),
                rec.clone()
            ),
            (
                "application.review".into(),
                "job_application".into(),
                a.clone(),
                rec.clone()
            ),
        ]
    );
    let ip = common::bytes_de(
        &e.m.db,
        "SELECT ip_hash FROM access_audit WHERE action = 'application.list'",
    )
    .await;
    assert!(!ip.is_empty());
    assert!(!common::contiene(&ip, b"203.0.113.9"));
    // Un 404 o un 403 no dejan fila: no se devolvió ningún dato.
    lista(&e, &e.rec2, &e.oferta, "").await;
    assert_eq!(auditoria(&e).await.len(), 3);
    e.m.db.cleanup().await;
}

/// PA8: si la fila de auditoría no se puede escribir, la petición falla y el dato no sale.
#[tokio::test]
async fn sin_auditoria_no_hay_dato() {
    let e = escenario().await;
    let a = aplicar(
        &e,
        &e.ana,
        &e.oferta,
        &["display_name", "skills", "experience", "email"],
    )
    .await;
    e.m.db
        .exec(
            "CREATE FUNCTION auditoria_rota() RETURNS trigger AS $$ \
             BEGIN RAISE EXCEPTION 'auditoria rota'; END; $$ LANGUAGE plpgsql; \
             CREATE TRIGGER auditoria_rota BEFORE INSERT ON access_audit \
             FOR EACH ROW EXECUTE FUNCTION auditoria_rota();",
        )
        .await
        .unwrap();
    for r in [
        lista(&e, &e.rec, &e.oferta, "").await,
        e.m.get(&detalle_de(&a), &e.rec).await,
        mover(&e, &e.rec, &a, json!({"to": "under_review"})).await,
    ] {
        assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", r.text);
        assert!(!r.text.contains("Ana"));
        assert!(!r.text.contains(&e.ana.email));
    }
    // El cambio de estado no quedó a medias.
    assert_eq!(
        e.m.n(&format!(
            "SELECT count(*) FROM job_applications WHERE id = '{a}' AND status = 'applied'"
        ))
        .await,
        1
    );
    assert_eq!(
        e.m.n(&format!(
            "SELECT count(*) FROM job_application_events WHERE job_application_id = '{a}'"
        ))
        .await,
        1
    );
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ PA9, orden y paginación

/// PA9: el panel no filtra ni ordena por nada que identifique a la persona; un parámetro fuera
/// del conjunto → 422 `parametro_no_permitido` sin repetirlo.
#[tokio::test]
async fn pa9_parametros_fuera_del_conjunto() {
    let e = escenario().await;
    aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    for q in [
        "age=30",
        "gender=f",
        "marital_status=soltera",
        "name=ana",
        "photo=si",
        "q=rust",
        "skill_id=x",
        "sort=name",
        "sort=age",
        "sort=match",
        "offset=2",
        "limit=5&gender=f",
    ] {
        let r = lista(&e, &e.rec, &e.oferta, q).await;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{q}: {}",
            r.text
        );
        assert_eq!(codigo(&r), "parametro_no_permitido", "{q}");
        for trozo in q.split(['&', '=']) {
            if trozo.len() > 3 && trozo != "limit" {
                assert!(!r.text.contains(trozo), "repite {trozo:?}");
            }
        }
    }
    for q in [
        "status=foo",
        "favorite=si",
        "favorite=",
        "sort=",
        "limit=0",
        "limit=51",
        "cursor=basura",
        "status=applied&status=hired",
    ] {
        let r = lista(&e, &e.rec, &e.oferta, q).await;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{q}: {}",
            r.text
        );
        assert_eq!(codigo(&r), "datos_invalidos", "{q}");
    }
    // Los parámetros se validan antes que el 404.
    let r = lista(&e, &e.rec, ID_NO, "gender=f").await;
    assert_eq!(codigo(&r), "parametro_no_permitido");
    e.m.db.cleanup().await;
}

/// Orden por fecha (`recent` por defecto, `oldest`), filtro por estado y cursor sin repetir ni
/// saltar filas.
#[tokio::test]
async fn orden_filtros_y_paginacion() {
    let e = escenario().await;
    let mut todas = Vec::new();
    for tag in ["c1", "c2", "c3", "c4"] {
        let p = e.m.persona(tag).await;
        todas.push(aplicar(&e, &p, &e.oferta, &MINIMO).await);
    }
    // Fechas distintas y conocidas: la primera es la más vieja.
    for (i, id) in todas.iter().enumerate() {
        e.m.db
            .exec(&format!(
                "UPDATE job_applications SET applied_at = now() - interval '{} hours' WHERE id = '{id}'",
                10 - i
            ))
            .await
            .unwrap();
    }
    let mut recientes = todas.clone();
    recientes.reverse();
    assert_eq!(ids(&lista(&e, &e.rec, &e.oferta, "").await), recientes);
    assert_eq!(
        ids(&lista(&e, &e.rec, &e.oferta, "sort=recent").await),
        recientes
    );
    assert_eq!(
        ids(&lista(&e, &e.rec, &e.oferta, "sort=oldest").await),
        todas
    );

    for (sort, esperado) in [("recent", &recientes), ("oldest", &todas)] {
        let mut vistas = Vec::new();
        let mut cursor = String::new();
        loop {
            let r = lista(
                &e,
                &e.rec,
                &e.oferta,
                &format!("sort={sort}&limit=3{cursor}"),
            )
            .await;
            assert_eq!(r.status, StatusCode::OK, "{}", r.text);
            vistas.extend(ids(&r));
            match r.json["next_cursor"].as_str() {
                Some(c) => cursor = format!("&cursor={c}"),
                None => break,
            }
        }
        assert_eq!(&vistas, esperado, "{sort}");
    }

    assert_eq!(
        mover(&e, &e.rec, &todas[1], json!({"to": "under_review"}))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        ids(&lista(&e, &e.rec, &e.oferta, "status=under_review").await),
        vec![todas[1].clone()]
    );
    assert_eq!(
        ids(&lista(&e, &e.rec, &e.oferta, "status=applied&sort=oldest").await),
        vec![todas[0].clone(), todas[2].clone(), todas[3].clone()]
    );
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ PA10, BO1, retención

/// PA10/BO2: quien borra su cuenta queda en el panel como entrada anónima, con su estado,
/// fechas e historial, sin datos. BO1: sus favoritos desaparecen.
#[tokio::test]
async fn pa10_el_candidato_eliminado_queda_anonimo() {
    let e = escenario().await;
    let a = aplicar(
        &e,
        &e.ana,
        &e.oferta,
        &["display_name", "skills", "experience", "email"],
    )
    .await;
    assert_eq!(
        mover(&e, &e.rec, &a, json!({"to": "under_review"}))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        favorito(&e, &e.rec, "PUT", &a).await.status,
        StatusCode::NO_CONTENT
    );
    let r =
        e.m.enviar(
            "DELETE",
            "/api/v1/me",
            Some(&e.ana.token),
            Some(json!({"password": CLAVE})),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    assert_eq!(
        e.m.n("SELECT count(*) FROM company_favorite_applicants")
            .await,
        0
    );

    let r = lista(&e, &e.rec, &e.oferta, "").await;
    let item = &r.json["items"][0];
    assert_eq!(item["availability"], "deleted");
    assert_eq!(item["applicant"], Value::Null);
    assert_eq!(item["status"], "under_review");
    assert_eq!(item["is_favorite"], false);
    assert!(item.get("shared_fields").is_none());

    let r = e.m.get(&detalle_de(&a), &e.rec).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json["availability"], "deleted");
    assert_eq!(r.json["applicant"], Value::Null);
    assert!(r.json.get("shared_fields").is_none());
    assert_eq!(r.json["history"].as_array().unwrap().len(), 2);
    assert!(!r.text.contains("Ana"));
    assert!(!r.text.contains(&e.ana.email));
    // Una entrada anónima no se mueve ni se marca.
    let r = mover(&e, &e.rec, &a, json!({"to": "shortlisted"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text);
    let r = favorito(&e, &e.rec, "PUT", &a).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text);
    e.m.db.cleanup().await;
}

/// AP8: la retirada sigue en el panel, sin datos; decisión 5: a los 90 días del cierre, igual.
#[tokio::test]
async fn retirada_y_retencion_vencida_no_muestran_datos() {
    let e = escenario().await;
    let a = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    let b = aplicar(&e, &e.beto, &e.oferta, &MINIMO).await;
    retirar(&e, &e.ana, &a).await;
    let r = e.m.get(&detalle_de(&a), &e.rec).await;
    assert_eq!(r.json["availability"], "withdrawn");
    assert_eq!(r.json["applicant"], Value::Null);
    assert_eq!(r.json["status"], "withdrawn");

    e.m.db
        .exec(&format!(
            "UPDATE job_offers SET status = 'closed', closed_at = now() - interval '89 days' WHERE id = '{}'",
            e.oferta
        ))
        .await
        .unwrap();
    let r = e.m.get(&detalle_de(&b), &e.rec).await;
    assert_eq!(r.json["availability"], "available");
    e.m.db
        .exec(&format!(
            "UPDATE job_offers SET closed_at = now() - interval '90 days 1 second' WHERE id = '{}'",
            e.oferta
        ))
        .await
        .unwrap();
    let r = e.m.get(&detalle_de(&b), &e.rec).await;
    assert_eq!(r.json["availability"], "retention_expired");
    assert_eq!(r.json["applicant"], Value::Null);
    let r = lista(&e, &e.rec, &e.oferta, "").await;
    for item in r.json["items"].as_array().unwrap() {
        assert_eq!(item["applicant"], Value::Null);
    }
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ empresa, permisos, sesión

/// La empresa sin verificar o con la membresía vencida no usa el panel (403), pero el 404 de lo
/// ajeno va antes.
#[tokio::test]
async fn sin_verificar_o_vencida_403_despues_del_404() {
    let e = escenario().await;
    let a = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    for (sql, error) in [
        (
            format!("UPDATE companies SET verified_at = NULL WHERE id = '{}'", e.emp),
            "empresa_no_verificada",
        ),
        (
            format!(
                "UPDATE companies SET verified_at = now() WHERE id = '{0}'; \
                 UPDATE memberships SET start_date = current_date - 100, expiration_date = current_date - 1 \
                 WHERE company_id = '{0}'",
                e.emp
            ),
            "membresia_vencida",
        ),
    ] {
        e.m.db.exec(&sql).await.unwrap();
        for r in [
            lista(&e, &e.rec, &e.oferta, "").await,
            e.m.get(&detalle_de(&a), &e.rec).await,
            mover(&e, &e.rec, &a, json!({"to": "under_review"})).await,
            favorito(&e, &e.rec, "PUT", &a).await,
            favorito(&e, &e.rec, "DELETE", &a).await,
        ] {
            assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.text);
            assert_eq!(codigo(&r), error);
            assert!(!r.text.contains("Ana"));
        }
        let r = lista(&e, &e.rec, ID_NO, "").await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "el 404 va antes que el 403");
    }
    assert_eq!(auditoria(&e).await.len(), 0);
    e.m.db.cleanup().await;
}

/// RO1: sin el permiso → 403 `sin_permiso` (la tabla de rutas); con el rol pero sin empresa → 404;
/// el admin de la plataforma no lee aplicaciones.
#[tokio::test]
async fn permisos_del_panel() {
    let e = escenario().await;
    let a = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    let admin = e.m.admin("plataforma").await;
    for p in [&e.beto, &admin] {
        for r in [
            lista(&e, p, &e.oferta, "").await,
            e.m.get(&detalle_de(&a), p).await,
            mover(&e, p, &a, json!({"to": "under_review"})).await,
            favorito(&e, p, "PUT", &a).await,
        ] {
            assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.text);
            assert_eq!(codigo(&r), "sin_permiso");
        }
    }
    let suelto = e.m.persona("suelto").await;
    e.m.dar_rol(&suelto.id, "recruiter").await;
    let r = lista(&e, &suelto, &e.oferta, "").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.text);
    let r = e.m.get(&detalle_de(&a), &suelto).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.text);
    e.m.db.cleanup().await;
}

/// SX3: sin token → 401 en las cinco rutas.
#[tokio::test]
async fn sin_token_401() {
    let e = escenario().await;
    let a = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    for (metodo, ruta) in [
        ("GET", lista_de(&e.oferta)),
        ("GET", detalle_de(&a)),
        ("POST", format!("{}/status", detalle_de(&a))),
        ("PUT", format!("{}/favorite", detalle_de(&a))),
        ("DELETE", format!("{}/favorite", detalle_de(&a))),
    ] {
        let r = e.m.enviar(metodo, &ruta, None, None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{metodo} {ruta}");
    }
    e.m.db.cleanup().await;
}

/// SX5: 120 detalles por reclutador y hora; el 121 → 429 con `Retry-After` y sin dato. Las
/// lecturas de hace más de una hora no cuentan, ni las de otro reclutador.
#[tokio::test]
async fn ciento_veinte_detalles_por_hora() {
    let e = escenario().await;
    let a = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    let rec_b = reclutador_sql(&e.m, "rec-b", &e.emp).await;
    e.m.db
        .exec(&format!(
            "INSERT INTO access_audit (id, actor_user_id, action, resource_type, resource_id, created_at) \
             SELECT gen_random_uuid(), '{}', 'application.view', 'job_application', '{a}', \
                    now() - interval '50 minutes' FROM generate_series(1, 119); \
             INSERT INTO access_audit (id, actor_user_id, action, resource_type, resource_id, created_at) \
             SELECT gen_random_uuid(), '{}', 'application.view', 'job_application', '{a}', \
                    now() - interval '61 minutes' FROM generate_series(1, 50)",
            e.rec.id, e.rec.id
        ))
        .await
        .unwrap();
    let r = e.m.get(&detalle_de(&a), &e.rec).await;
    assert_eq!(r.status, StatusCode::OK, "la 120 pasa: {}", r.text);
    let r = e.m.get(&detalle_de(&a), &e.rec).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.text);
    assert_eq!(codigo(&r), "demasiados_intentos");
    let espera: u64 = r.headers["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=3600).contains(&espera), "{espera}");
    assert!(!r.text.contains("Ana"));
    // Otro reclutador de la misma empresa tiene su propio cupo.
    let r = e.m.get(&detalle_de(&a), &rec_b).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    e.m.db.cleanup().await;
}

/// SX4: nada de la persona ni de la nota llega a un log.
#[tokio::test]
async fn nada_del_panel_llega_al_log() {
    let e = escenario().await;
    let a = aplicar(
        &e,
        &e.ana,
        &e.oferta,
        &[
            "display_name",
            "skills",
            "experience",
            "email",
            "phone",
            "profile_description",
        ],
    )
    .await;
    let (registro, _guarda) = capturar_log();
    lista(&e, &e.rec, &e.oferta, "").await;
    e.m.get(&detalle_de(&a), &e.rec).await;
    mover(
        &e,
        &e.rec,
        &a,
        json!({"to": "under_review", "note": SECRETO_NOTA}),
    )
    .await;
    mover(&e, &e.rec, &a, json!({"to": "hired", "note": SECRETO_NOTA})).await;
    favorito(&e, &e.rec, "PUT", &a).await;
    let log = registro.texto();
    assert!(log.contains("captura-activa"), "la captura no funciona");
    for prohibido in [
        e.ana.email.as_str(),
        SECRETO_TEL,
        SECRETO_PERFIL,
        SECRETO_NOTA,
        "Ana P.",
        "Banco X",
    ] {
        assert!(!log.contains(prohibido), "el log trae {prohibido:?}");
    }
    e.m.db.cleanup().await;
}

/// Todos los reclutadores de la empresa ven las aplicaciones de todas sus ofertas.
#[tokio::test]
async fn cualquier_reclutador_de_la_empresa_ve_el_panel() {
    let e = escenario().await;
    let a = aplicar(&e, &e.ana, &e.oferta, &MINIMO).await;
    let colega = reclutador_sql(&e.m, "colega", &e.emp).await;
    assert_eq!(
        ids(&lista(&e, &colega, &e.oferta, "").await),
        vec![a.clone()]
    );
    assert_eq!(
        e.m.get(&detalle_de(&a), &colega).await.status,
        StatusCode::OK
    );
    let _ = &e.emp2;
    e.m.db.cleanup().await;
}

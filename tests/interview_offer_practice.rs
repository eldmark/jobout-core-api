//! Casos de verdad de la práctica de una oferta (Fase 5b, paso 9, HU-31: PR1–PR5 y lo que toca de
//! PR6), contra «Entrevista (Fase 3)» y «Práctica de una oferta (Fase 5b)» de `api.md`. PostgreSQL
//! real, una base por prueba. Se escribieron antes que el código, en rojo.
//!
//! Empresas y ofertas se insertan por SQL. La cuota del plan está **activa** (no se apaga): cada
//! prueba usa una persona distinta salvo la que mide la cuota.

#[allow(dead_code)]
mod common;
#[path = "common/empresas.rs"]
mod empresas;

use axum::http::StatusCode;
use empresas::{mundo, Mundo, Persona, Resp};
use serde_json::{json, Value};

const BUENA: &str = "En mi proyecto final de la universidad usamos Rust con PostgreSQL y yo me encargué de las consultas lentas, porque la página tardaba 800 ms en responder. Primero medí el problema, después añadí un índice y reescribí dos consultas, y como resultado el tiempo bajó a 200 ms y atendimos a 300 usuarios a la vez.";
const ID_NO: &str = "00000000-0000-7000-8000-000000000000";

// ------------------------------------------------------------------ fixtures

async fn empresa(m: &Mundo, nombre: &str) -> String {
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

async fn oferta(m: &Mundo, company: &str, titulo: &str, confidencial: bool) -> String {
    let id = m.texto("SELECT gen_random_uuid()::text").await;
    m.db.exec(&format!(
        "INSERT INTO job_offers (id, company_id, name, description, status, is_confidential, publication_date) \
         VALUES ('{id}', '{company}', '{titulo}', 'Una oferta.', 'published', {confidencial}, current_date)"
    ))
    .await
    .unwrap();
    id
}

/// `n`-ésima habilidad del catálogo (por código): (id, nombre).
async fn skill(m: &Mundo, n: usize) -> (String, String) {
    let id = m
        .texto(&format!(
            "SELECT id::text FROM skills ORDER BY code OFFSET {n} LIMIT 1"
        ))
        .await;
    let nombre = m
        .texto(&format!("SELECT name FROM skills WHERE id = '{id}'"))
        .await;
    (id, nombre)
}

async fn pide_skill(m: &Mundo, oferta: &str, skill: &str, obligatoria: bool) {
    m.db.exec(&format!(
        "INSERT INTO job_offer_skills (job_offer_id, skill_id, is_required) \
         VALUES ('{oferta}', '{skill}', {obligatoria})"
    ))
    .await
    .unwrap();
}

async fn idioma(m: &Mundo, oferta: &str) -> String {
    let (id, nombre) = (
        m.texto("SELECT id::text FROM languages ORDER BY code LIMIT 1")
            .await,
        m.texto("SELECT name FROM languages ORDER BY code LIMIT 1")
            .await,
    );
    m.db.exec(&format!(
        "INSERT INTO job_offer_languages (job_offer_id, language_id, min_proficiency) \
         VALUES ('{oferta}', '{id}', 'intermediate')"
    ))
    .await
    .unwrap();
    nombre
}

async fn recruiter_id(m: &Mundo, code: &str) -> String {
    m.texto(&format!(
        "SELECT id::text FROM ai_recruiters WHERE code = '{code}'"
    ))
    .await
}
async fn nivel_id(m: &Mundo, code: &str) -> String {
    m.texto(&format!(
        "SELECT id::text FROM interview_levels WHERE code = '{code}'"
    ))
    .await
}

fn cuerpo(r: &str, l: &str, oferta: Option<&str>) -> Value {
    let mut c = json!({"recruiter_id": r, "level_id": l});
    if let Some(o) = oferta {
        c["job_offer_id"] = json!(o);
    }
    c
}

async fn practicar(m: &Mundo, p: &Persona, oferta: Option<&str>) -> Resp {
    let (r, l) = (recruiter_id(m, "ana").await, nivel_id(m, "junior").await);
    m.enviar(
        "POST",
        "/api/v1/interviews",
        Some(&p.token),
        Some(cuerpo(&r, &l, oferta)),
    )
    .await
}

async fn practicar_ok(m: &Mundo, p: &Persona, oferta: &str) -> Value {
    let r = practicar(m, p, Some(oferta)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    r.json
}

async fn responder(m: &Mundo, p: &Persona, sesion: &str, texto: &str) -> Resp {
    m.enviar(
        "POST",
        &format!("/api/v1/interviews/{sesion}/turns"),
        Some(&p.token),
        Some(json!({"text": texto})),
    )
    .await
}

fn pregunta(entrevista: &Value) -> &str {
    entrevista["turns"][0]["text"].as_str().unwrap()
}

// ------------------------------------------------------------------ PR1: se guarda y se muestra

#[tokio::test]
async fn pr1_crear_con_oferta_guarda_el_vinculo_y_lo_muestra() {
    let m = mundo().await;
    let emp = empresa(&m, "cafenuevo").await;
    let o = oferta(&m, &emp, "Barista de turno", false).await;
    let ana = m.persona("ana").await;

    let e = practicar_ok(&m, &ana, &o).await;
    assert_eq!(
        e["offer"],
        json!({"id": o, "title": "Barista de turno", "company_name": "cafenuevo"})
    );
    assert!(
        pregunta(&e).contains("Barista de turno"),
        "{}",
        pregunta(&e)
    );
    let id = e["id"].as_str().unwrap();
    assert_eq!(
        m.n(&format!(
            "SELECT count(*) FROM interview_sessions WHERE id = '{id}' AND job_offer_id = '{o}'"
        ))
        .await,
        1
    );

    // La lectura y el listado la traen igual.
    let r = m.get(&format!("/api/v1/interviews/{id}"), &ana).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["offer"], e["offer"]);
    let r = m.get("/api/v1/interviews", &ana).await;
    assert_eq!(r.json["items"][0]["offer"], e["offer"]);
}

#[tokio::test]
async fn pr1_sin_oferta_no_hay_clave_offer_y_nada_cambia() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let r = practicar(&m, &ana, None).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    assert!(r.json.get("offer").is_none(), "{}", r.text);
    let id = r.json["id"].as_str().unwrap();
    assert_eq!(
        m.n(&format!(
            "SELECT count(*) FROM interview_sessions WHERE id = '{id}' AND job_offer_id IS NULL"
        ))
        .await,
        1
    );
    let l = m.get("/api/v1/interviews", &ana).await;
    assert!(l.json["items"][0].get("offer").is_none());
    // `null` explícito vale como ausente.
    let _ = m
        .enviar(
            "POST",
            &format!("/api/v1/interviews/{id}/abandon"),
            Some(&ana.token),
            None,
        )
        .await;
}

#[tokio::test]
async fn pr1_las_preguntas_siguientes_salen_de_los_requisitos_obligatorios() {
    let m = mundo().await;
    let emp = empresa(&m, "cafenuevo").await;
    let o = oferta(&m, &emp, "Analista", false).await;
    let (s1, n1) = skill(&m, 0).await;
    let (s2, n2) = skill(&m, 1).await;
    pide_skill(&m, &o, &s1, true).await;
    pide_skill(&m, &o, &s2, false).await; // deseable: no se pregunta
    let lang = idioma(&m, &o).await;
    let ana = m.persona("ana").await;

    let e = practicar_ok(&m, &ana, &o).await;
    let id = e["id"].as_str().unwrap().to_string();
    let mut preguntas = vec![pregunta(&e).to_string()];
    for _ in 0..12 {
        let r = responder(&m, &ana, &id, BUENA).await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
        preguntas.push(r.json["turn"]["text"].as_str().unwrap().to_string());
        if r.json["done"] == json!(true) {
            break;
        }
    }
    let cuenta = |h: &str| preguntas.iter().filter(|q| q.contains(h)).count();
    assert_eq!(cuenta(&n1), 1, "{preguntas:?}");
    assert_eq!(cuenta(&lang), 1, "{preguntas:?}");
    assert_eq!(
        cuenta(&n2),
        0,
        "una habilidad deseable no se pregunta: {preguntas:?}"
    );
}

#[tokio::test]
async fn pr1_un_id_que_no_es_cadena_es_422() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let (r, l) = (recruiter_id(&m, "ana").await, nivel_id(&m, "junior").await);
    for malo in [json!(7), json!(true), json!(["x"]), json!({"a": 1})] {
        let mut c = cuerpo(&r, &l, None);
        c["job_offer_id"] = malo;
        let r = m
            .enviar("POST", "/api/v1/interviews", Some(&ana.token), Some(c))
            .await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.text);
        assert_eq!(r.json["error"], "datos_invalidos");
    }
    let mut c = cuerpo(&r, &l, None);
    c["job_offer_id"] = Value::Null;
    let r = m
        .enviar("POST", "/api/v1/interviews", Some(&ana.token), Some(c))
        .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "null equivale a ausente: {}",
        r.text
    );
}

// ------------------------------------------------------------------ PR2: confidencial

#[tokio::test]
async fn pr2_una_oferta_confidencial_no_revela_la_empresa() {
    let m = mundo().await;
    let emp = empresa(&m, "ReservadaSA").await;
    let o = oferta(&m, &emp, "Gerente comercial", true).await;
    let ana = m.persona("ana").await;
    let e = practicar_ok(&m, &ana, &o).await;
    let id = e["id"].as_str().unwrap();

    assert_eq!(e["offer"]["id"], json!(o));
    assert_eq!(e["offer"]["title"], "Gerente comercial");
    assert_eq!(e["offer"]["company_name"], Value::Null);
    let r = responder(&m, &ana, id, BUENA).await;
    let lectura = m.get(&format!("/api/v1/interviews/{id}"), &ana).await;
    let lista = m.get("/api/v1/interviews", &ana).await;
    for t in [e.to_string(), r.text.clone(), lectura.text, lista.text] {
        assert!(!t.to_lowercase().contains("reservadasa"), "{t}");
    }
}

// ------------------------------------------------------------------ PR4: no visible = 404

#[tokio::test]
async fn pr4_toda_oferta_no_visible_da_el_mismo_404() {
    let m = mundo().await;
    let emp = empresa(&m, "cafenuevo").await;
    let ana = m.persona("ana").await;

    let borrador = oferta(&m, &emp, "Borrador", false).await;
    m.db.exec(&format!(
        "UPDATE job_offers SET status='draft', publication_date=NULL WHERE id='{borrador}'"
    ))
    .await
    .unwrap();
    let pausada = oferta(&m, &emp, "Pausada", false).await;
    m.db.exec(&format!(
        "UPDATE job_offers SET status='paused' WHERE id='{pausada}'"
    ))
    .await
    .unwrap();
    let cerrada = oferta(&m, &emp, "Cerrada", false).await;
    m.db.exec(&format!(
        "UPDATE job_offers SET status='closed', closed_at=now() WHERE id='{cerrada}'"
    ))
    .await
    .unwrap();
    let vencida = oferta(&m, &emp, "Vencida", false).await;
    m.db.exec(&format!(
        "UPDATE job_offers SET hiring_date=current_date-1 WHERE id='{vencida}'"
    ))
    .await
    .unwrap();

    let sin_verificar = empresa(&m, "sinverificar").await;
    m.db.exec(&format!(
        "UPDATE companies SET verified_at=NULL WHERE id='{sin_verificar}'"
    ))
    .await
    .unwrap();
    let o_sin_verificar = oferta(&m, &sin_verificar, "SinVerificar", false).await;
    let membresia = empresa(&m, "membresiavencida").await;
    m.db.exec(&format!(
        "UPDATE memberships SET expiration_date=current_date-1 WHERE company_id='{membresia}'"
    ))
    .await
    .unwrap();
    let o_membresia = oferta(&m, &membresia, "MembresiaVencida", false).await;
    let inactiva = empresa(&m, "inactiva").await;
    m.db.exec(&format!(
        "UPDATE companies SET active=false WHERE id='{inactiva}'"
    ))
    .await
    .unwrap();
    let o_inactiva = oferta(&m, &inactiva, "Inactiva", false).await;
    let borrada = empresa(&m, "borrada").await;
    let o_borrada = oferta(&m, &borrada, "Borrada", false).await;
    m.db.exec(&format!(
        "UPDATE companies SET deleted_at=now() WHERE id='{borrada}'"
    ))
    .await
    .unwrap();

    let referencia = practicar(&m, &ana, Some(ID_NO)).await;
    assert_eq!(
        referencia.status,
        StatusCode::NOT_FOUND,
        "{}",
        referencia.text
    );
    assert_eq!(referencia.json["error"], "no_encontrado");

    let casos = [
        borrador,
        pausada,
        cerrada,
        vencida,
        o_sin_verificar,
        o_membresia,
        o_inactiva,
        o_borrada,
        "no-es-un-uuid".to_string(),
        "".to_string(),
    ];
    for oferta in &casos {
        let r = practicar(&m, &ana, Some(oferta)).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{oferta}: {}", r.text);
        assert_eq!(r.json, referencia.json, "{oferta}");
    }
    // Ninguno dejó sesión ni gastó cuota.
    assert_eq!(m.n("SELECT count(*) FROM interview_sessions").await, 0);
    assert_eq!(m.n("SELECT count(*) FROM usage_ledger").await, 0);
}

#[tokio::test]
async fn pr4_el_orden_de_validacion_pone_la_oferta_con_los_404() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let (r, l) = (recruiter_id(&m, "ana").await, nivel_id(&m, "junior").await);
    let amazon = nivel_id(&m, "amazon").await;
    let post = |c: Value| m.enviar("POST", "/api/v1/interviews", Some(&ana.token), Some(c));

    // Cuerpo inválido (422) gana a la oferta inexistente (404).
    let mut c = cuerpo("basura", &l, Some(ID_NO));
    assert_eq!(
        post(c.clone()).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Recruiter inexistente y oferta inexistente: 404 igual.
    c = cuerpo(ID_NO, &l, Some(ID_NO));
    assert_eq!(post(c).await.status, StatusCode::NOT_FOUND);
    // Oferta inexistente antes que «nivel no disponible» (409).
    let c = cuerpo(&r, &amazon, Some(ID_NO));
    assert_eq!(post(c).await.status, StatusCode::NOT_FOUND);
    // Y antes que «sesión en curso» (409).
    let abierta = practicar(&m, &ana, None).await;
    assert_eq!(abierta.status, StatusCode::CREATED);
    let c = cuerpo(&r, &l, Some(ID_NO));
    assert_eq!(post(c).await.status, StatusCode::NOT_FOUND);
    // Con oferta visible, la sesión en curso sigue mandando.
    let emp = empresa(&m, "cafenuevo").await;
    let o = oferta(&m, &emp, "Barista", false).await;
    let r2 = post(cuerpo(&r, &l, Some(&o))).await;
    assert_eq!(r2.status, StatusCode::CONFLICT);
    assert_eq!(r2.json["error"], "sesion_en_curso");
}

// ------------------------------------------------------------------ PR3: no es una aplicación

#[tokio::test]
async fn pr3_practicar_no_es_aplicar_ni_la_ve_la_empresa() {
    let m = mundo().await;
    let emp = empresa(&m, "cafenuevo").await;
    let o = oferta(&m, &emp, "Barista", false).await;
    let rec = m.persona("rec").await;
    m.dar_rol(&rec.id, "recruiter").await;
    m.db.exec(&format!(
        "INSERT INTO company_recruiters (id, user_id, company_id, role) \
         VALUES (gen_random_uuid(), '{}', '{emp}', 'recruiter')",
        rec.id
    ))
    .await
    .unwrap();
    let ana = m.persona("ana").await;
    let e = practicar_ok(&m, &ana, &o).await;
    let id = e["id"].as_str().unwrap();
    assert_eq!(
        responder(&m, &ana, id, BUENA).await.status,
        StatusCode::CREATED
    );

    assert_eq!(m.n("SELECT count(*) FROM job_applications").await, 0);
    // El panel no lista nada.
    let panel = m
        .get(&format!("/api/v1/company/offers/{o}/applications"), &rec)
        .await;
    assert_eq!(panel.status, StatusCode::OK, "{}", panel.text);
    assert_eq!(panel.json["items"], json!([]));
    // Ni la oferta del reclutador ni nada de lo que lee lleva la sesión o a la persona.
    let det = m.get(&format!("/api/v1/company/offers/{o}"), &rec).await;
    assert_eq!(det.status, StatusCode::OK, "{}", det.text);
    for t in [&panel.text, &det.text] {
        assert!(!t.contains(id), "{t}");
        assert!(!t.contains(&ana.id), "{t}");
        assert!(!t.contains(&ana.email), "{t}");
    }
    // Y la candidata aún puede aplicar: practicar no la dejó «ya aplicada».
    let r = m.get(&format!("/api/v1/offers/{o}"), &ana).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert!(r.json["my_application"].is_null(), "{}", r.text);
    // Un reclutador ajeno a la sesión no la puede leer (404, como cualquier sesión ajena).
    let r = m.get(&format!("/api/v1/interviews/{id}"), &rec).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------------ PR5: cuota y sesión activa

#[tokio::test]
async fn pr5_practicar_gasta_la_cuota_y_respeta_la_sesion_activa() {
    let m = mundo().await;
    let emp = empresa(&m, "cafenuevo").await;
    let o = oferta(&m, &emp, "Barista", false).await;
    let ana = m.persona("ana").await;

    let e = practicar_ok(&m, &ana, &o).await;
    let id = e["id"].as_str().unwrap();
    assert_eq!(m.n("SELECT count(*) FROM usage_ledger").await, 1);

    // Una sesión activa: la segunda práctica (de esa u otra oferta) es 409 con `session_id`.
    let r = practicar(&m, &ana, Some(&o)).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text);
    assert_eq!(r.json["error"], "sesion_en_curso");
    assert_eq!(r.json["session_id"], json!(id));

    // Abandonar no devuelve la cuota (plan Gratis: 1 al mes).
    let r = m
        .enviar(
            "POST",
            &format!("/api/v1/interviews/{id}/abandon"),
            Some(&ana.token),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = practicar(&m, &ana, Some(&o)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.text);
    assert_eq!(r.json["error"], "limite_del_plan");
    assert!(r.json["renews_at"].is_string());
    assert_eq!(m.n("SELECT count(*) FROM usage_ledger").await, 1);
}

// ------------------------------------------------------------------ SET NULL: la sesión sobrevive

#[tokio::test]
async fn borrar_la_oferta_deja_la_sesion_sin_vinculo_y_sigue_funcionando() {
    let m = mundo().await;
    let emp = empresa(&m, "cafenuevo").await;
    let o = oferta(&m, &emp, "Barista", false).await;
    let ana = m.persona("ana").await;
    let e = practicar_ok(&m, &ana, &o).await;
    let id = e["id"].as_str().unwrap();

    m.db.exec(&format!("DELETE FROM job_offers WHERE id='{o}'"))
        .await
        .unwrap();
    assert_eq!(
        m.n(&format!(
            "SELECT count(*) FROM interview_sessions WHERE id='{id}' AND job_offer_id IS NULL"
        ))
        .await,
        1
    );
    let r = m.get(&format!("/api/v1/interviews/{id}"), &ana).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert!(r.json.get("offer").is_none(), "{}", r.text);
    assert_eq!(r.json["turns"].as_array().unwrap().len(), 1);
    // La conversación sigue (con el banco base).
    assert_eq!(
        responder(&m, &ana, id, BUENA).await.status,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn cerrar_la_oferta_conserva_la_sesion_y_su_vinculo() {
    let m = mundo().await;
    let emp = empresa(&m, "cafenuevo").await;
    let o = oferta(&m, &emp, "Barista", false).await;
    let ana = m.persona("ana").await;
    let e = practicar_ok(&m, &ana, &o).await;
    let id = e["id"].as_str().unwrap();

    m.db.exec(&format!(
        "UPDATE job_offers SET status='closed', closed_at=now() WHERE id='{o}'"
    ))
    .await
    .unwrap();
    let r = m.get(&format!("/api/v1/interviews/{id}"), &ana).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["offer"]["id"], json!(o));
    assert_eq!(r.json["offer"]["title"], "Barista");
    assert_eq!(
        responder(&m, &ana, id, BUENA).await.status,
        StatusCode::CREATED
    );
}

// ------------------------------------------------------------------ ruta y permisos

#[tokio::test]
async fn sin_token_sigue_siendo_401() {
    let m = mundo().await;
    let r = m
        .enviar(
            "POST",
            "/api/v1/interviews",
            None,
            Some(json!({"job_offer_id": ID_NO})),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

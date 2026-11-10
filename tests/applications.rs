//! Casos de verdad de las aplicaciones con consentimiento explícito (Fase 5a, paso 6, HU-52,
//! AP1–AP8, PA5, PA10, BO1, BO2, SX2, SX5, SX6), contra «Aplicar, ver y retirar (candidato)» de
//! `api.md`. PostgreSQL real, una base por prueba. Se escribieron antes que el código, en rojo.
//!
//! Empresas, ofertas y perfiles se insertan por SQL: estas pruebas no dependen de otras rutas.
//! Ningún reloj de pared en las pruebas de retención: el instante entra como parámetro.

mod common;
#[path = "common/empresas.rs"]
mod empresas;

use axum::http::StatusCode;
use empresas::{capturar_log, mundo, pii, Mundo, Persona, Resp, CLAVE};
use jobout_core_api::applications::{move_status, shared_view, MoveError, Scope, Status};
use serde_json::{json, Value};

const ID_NO: &str = "00000000-0000-7000-8000-000000000000";
const MINIMO: [&str; 3] = ["display_name", "skills", "experience"];

// ------------------------------------------------------------------ fixtures

async fn empresa_sql(m: &Mundo, nombre: &str, verificada: bool, vigente: bool) -> String {
    let id = m.texto("SELECT gen_random_uuid()::text").await;
    let pais = m
        .texto("SELECT id::text FROM countries ORDER BY code LIMIT 1")
        .await;
    let sector = m
        .texto("SELECT id::text FROM employment_sectors ORDER BY code LIMIT 1")
        .await;
    m.db.exec(&format!(
        "INSERT INTO companies (id, name, tax_id, tax_id_hash, website, country_id, employment_sector_id, verified_at) \
         VALUES ('{id}', '{nombre}', '\\x01'::bytea, '\\x{}'::bytea, 'https://{nombre}.example', '{pais}', '{sector}', {})",
        common::hex(nombre.as_bytes()),
        if verificada { "now()" } else { "NULL" }
    ))
    .await
    .unwrap();
    let tipo = m
        .texto("SELECT id::text FROM membership_types ORDER BY code LIMIT 1")
        .await;
    let (ini, fin) = if vigente {
        ("current_date - 10", "current_date + 100")
    } else {
        ("current_date - 100", "current_date - 1")
    };
    m.db.exec(&format!(
        "INSERT INTO memberships (id, company_id, membership_type_id, start_date, expiration_date, status) \
         VALUES (gen_random_uuid(), '{id}', '{tipo}', {ini}, {fin}, 'active')"
    ))
    .await
    .unwrap();
    id
}

async fn oferta_sql(
    m: &Mundo,
    company: &str,
    estado: &str,
    confidencial: bool,
    cierre: &str,
) -> String {
    let id = m.texto("SELECT gen_random_uuid()::text").await;
    let estado_sql = estado;
    let closed = if estado == "closed" {
        format!("now() - interval '{cierre}'")
    } else {
        "NULL".to_string()
    };
    m.db.exec(&format!(
        "INSERT INTO job_offers (id, company_id, name, description, status, is_confidential, closed_at, \
            publication_date, state_id, city_id) \
         VALUES ('{id}', '{company}', 'Desarrollador backend', 'Una oferta.', '{estado_sql}', {confidencial}, {closed}, \
            current_date, (SELECT id FROM states WHERE code = 'guatemala'), \
            (SELECT c.id FROM cities c JOIN states s ON s.id = c.state_id WHERE s.code = 'guatemala' ORDER BY c.code LIMIT 1))"
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
const SECRETO_DESC: &str = "SECRETO-DESCRIPCION-DE-EXPERIENCIA";
const SECRETO_PERFIL: &str = "SECRETO-PERFIL-PROFESIONAL";
const SECRETO_CASADA: &str = "SECRETO-CASADA";
const SECRETO_APELLIDO2: &str = "SECRETO-SEGUNDO";

/// Perfil completo de la persona, con datos que **nunca** deben salir sin consentimiento.
async fn perfil_sql(m: &Mundo, p: &Persona) {
    let a = m
        .texto(&format!(
            "SELECT id::text FROM applicants WHERE user_id = '{}'",
            p.id
        ))
        .await;
    let llaves = pii();
    let hex = |t: &str| common::hex(&llaves.encrypt(t).unwrap());
    m.db.exec(&format!(
        "UPDATE users SET married_last_name = '{SECRETO_CASADA}', last_name2 = '{SECRETO_APELLIDO2}', \
            middle_name = 'SECRETO-MEDIO' WHERE id = '{}'",
        p.id
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "INSERT INTO professional_data (id, applicant_id, description, last_academic_level_id, profession_id, last_job_hierarchy_id) \
         VALUES (gen_random_uuid(), '{a}', '{SECRETO_PERFIL}', (SELECT id FROM academic_levels ORDER BY code LIMIT 1), \
                 (SELECT id FROM professions ORDER BY code LIMIT 1), (SELECT id FROM job_hierarchies ORDER BY code LIMIT 1))"
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "INSERT INTO applicant_languages (id, applicant_id, language_id, proficiency) \
         VALUES (gen_random_uuid(), '{a}', (SELECT id FROM languages ORDER BY code LIMIT 1), 'advanced')"
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
        "INSERT INTO applicant_experiences (id, applicant_id, company, title, start_date, end_date, description, position) \
         VALUES (gen_random_uuid(), '{a}', 'Banco X', 'Analista', '2022-01-01', NULL, '\\x{}'::bytea, 0)",
        hex(SECRETO_DESC)
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "INSERT INTO personal_data (id, applicant_id, phone) VALUES (gen_random_uuid(), '{a}', '\\x{}'::bytea)",
        hex(SECRETO_TEL)
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
}

async fn escenario() -> Esc {
    let m = mundo().await;
    let emp = empresa_sql(&m, "cafenuevo", true, true).await;
    let oferta = oferta_sql(&m, &emp, "published", false, "").await;
    let ana = m.persona("ana").await;
    let beto = m.persona("beto").await;
    let rec = reclutador_sql(&m, "rec", &emp).await;
    perfil_sql(&m, &ana).await;
    Esc {
        m,
        ana,
        beto,
        rec,
        emp,
        oferta,
    }
}

fn aplicar_cuerpo(campos: &[&str]) -> Value {
    json!({"consent": true, "shared_fields": campos})
}

async fn aplicar(e: &Esc, p: &Persona, oferta: &str, cuerpo: Value) -> Resp {
    e.m.enviar(
        "POST",
        &format!("/api/v1/offers/{oferta}/applications"),
        Some(&p.token),
        Some(cuerpo),
    )
    .await
}

async fn aplicar_ok(e: &Esc, p: &Persona, oferta: &str, campos: &[&str]) -> Value {
    let r = aplicar(e, p, oferta, aplicar_cuerpo(campos)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    r.json
}

fn codigo(r: &Resp) -> &str {
    r.json["error"].as_str().unwrap_or("")
}

async fn ahora_micros(m: &Mundo) -> i64 {
    m.texto("SELECT ((extract(epoch from now()) * 1000000)::bigint)::text")
        .await
        .parse()
        .unwrap()
}

async fn mover(e: &Esc, id: &str, a: Status) -> Result<(), MoveError> {
    move_status(
        &e.m.db.conn,
        &pii(),
        Scope::Company {
            company_id: &e.emp,
            user_id: &e.rec.id,
        },
        id,
        a,
        None,
    )
    .await
    .map(|_| ())
}

async fn estado_de(e: &Esc, id: &str) -> String {
    e.m.texto(&format!(
        "SELECT status::text FROM job_applications WHERE id = '{id}'"
    ))
    .await
}

// ------------------------------------------------------------------ consentimiento

/// AP1: aplicar con consentimiento crea la aplicación, el evento y la fecha del servidor.
#[tokio::test]
async fn aplicar_con_consentimiento_crea_aplicacion_y_evento() {
    let e = escenario().await;
    let v = aplicar_ok(
        &e,
        &e.ana,
        &e.oferta,
        &["email", "experience", "skills", "display_name"],
    )
    .await;
    assert_eq!(v["status"], "applied");
    // Orden canónico, no el recibido.
    assert_eq!(
        v["shared_fields"],
        json!(["display_name", "skills", "experience", "email"])
    );
    for k in [
        "id",
        "applied_at",
        "status_changed_at",
        "consented_at",
        "offer",
        "history",
    ] {
        assert!(v.get(k).is_some(), "falta {k}");
    }
    assert_eq!(v["offer"]["title"], "Desarrollador backend");
    assert_eq!(v["offer"]["confidential"], false);
    assert_eq!(v["offer"]["company"]["name"], "cafenuevo");
    assert_eq!(v["history"].as_array().unwrap().len(), 1);
    assert_eq!(v["history"][0]["status"], "applied");
    assert_eq!(e.m.n("SELECT count(*) FROM job_applications").await, 1);
    assert_eq!(
        e.m.n(&format!(
            "SELECT count(*) FROM job_application_events WHERE from_status IS NULL AND to_status = 'applied' \
             AND changed_by = '{}'",
            e.ana.id
        ))
        .await,
        1
    );
    e.m.db.cleanup().await;
}

/// AP1 / HU-52: sin `consent: true` → 422 y no se crea nada, sea cual sea el tipo.
#[tokio::test]
async fn sin_consentimiento_422_y_no_crea_nada() {
    let e = escenario().await;
    for cuerpo in [
        json!({"shared_fields": MINIMO}),
        json!({"consent": false, "shared_fields": MINIMO}),
        json!({"consent": null, "shared_fields": MINIMO}),
        json!({"consent": "true", "shared_fields": MINIMO}),
        json!({"consent": 1, "shared_fields": MINIMO}),
    ] {
        let r = aplicar(&e, &e.ana, &e.oferta, cuerpo.clone()).await;
        assert_eq!(
            (r.status, codigo(&r)),
            (StatusCode::UNPROCESSABLE_ENTITY, "consentimiento_requerido"),
            "{cuerpo}"
        );
    }
    assert_eq!(e.m.n("SELECT count(*) FROM job_applications").await, 0);
    assert_eq!(
        e.m.n("SELECT count(*) FROM job_application_events").await,
        0
    );
    e.m.db.cleanup().await;
}

/// El consentimiento se comprueba antes que la oferta: ni siquiera dice si existe.
#[tokio::test]
async fn el_consentimiento_va_antes_que_el_404() {
    let e = escenario().await;
    let r = aplicar(
        &e,
        &e.ana,
        ID_NO,
        json!({"consent": false, "shared_fields": MINIMO}),
    )
    .await;
    assert_eq!(codigo(&r), "consentimiento_requerido");
    e.m.db.cleanup().await;
}

/// AP2: `shared_fields` cerrado, con los tres obligatorios.
#[tokio::test]
async fn shared_fields_invalidos_422() {
    let e = escenario().await;
    for campos in [
        json!(["display_name", "skills"]),
        json!(["display_name", "skills", "experience", "cv"]),
        json!(["display_name", "skills", "experience", "birth_date"]),
        json!(["display_name", "skills", "experience", "email", "email"]),
        json!(["display_name", "skills", "experience", 7]),
        json!("display_name"),
        json!([]),
    ] {
        let r = aplicar(
            &e,
            &e.ana,
            &e.oferta,
            json!({"consent": true, "shared_fields": campos}),
        )
        .await;
        assert_eq!(
            (r.status, codigo(&r)),
            (StatusCode::UNPROCESSABLE_ENTITY, "datos_invalidos"),
            "{campos}"
        );
    }
    let r = aplicar(
        &e,
        &e.ana,
        &e.oferta,
        json!({"consent": true, "shared_fields": MINIMO, "applicant_id": "x"}),
    )
    .await;
    assert_eq!(codigo(&r), "campo_no_permitido");
    assert_eq!(e.m.n("SELECT count(*) FROM job_applications").await, 0);
    e.m.db.cleanup().await;
}

/// `consented_at` lo fija el servidor: el cliente no lo puede mandar.
#[tokio::test]
async fn consented_at_no_lo_manda_el_cliente() {
    let e = escenario().await;
    let r = aplicar(
        &e,
        &e.ana,
        &e.oferta,
        json!({"consent": true, "shared_fields": MINIMO, "consented_at": "2020-01-01T00:00:00Z"}),
    )
    .await;
    assert_eq!(codigo(&r), "campo_no_permitido");
    e.m.db.cleanup().await;
}

/// Sin token → 401.
#[tokio::test]
async fn sin_token_401() {
    let e = escenario().await;
    let r =
        e.m.enviar(
            "POST",
            &format!("/api/v1/offers/{}/applications", e.oferta),
            None,
            Some(aplicar_cuerpo(&MINIMO)),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ visibilidad de la oferta

/// AP5, CA5: borrador, pausada, cerrada, empresa sin verificar, membresía vencida, inexistente
/// y mal formada dan el mismo 404, sin nombrar empresa.
#[tokio::test]
async fn oferta_no_visible_es_el_mismo_404() {
    let e = escenario().await;
    let sin_verificar = empresa_sql(&e.m, "sinverificar", false, true).await;
    let vencida = empresa_sql(&e.m, "vencida", true, false).await;
    let inactiva = empresa_sql(&e.m, "inactiva", true, true).await;
    e.m.db
        .exec(&format!(
            "UPDATE companies SET active = false WHERE id = '{inactiva}'"
        ))
        .await
        .unwrap();
    let borrada = empresa_sql(&e.m, "borrada", true, true).await;
    e.m.db
        .exec(&format!(
            "UPDATE companies SET deleted_at = now() WHERE id = '{borrada}'"
        ))
        .await
        .unwrap();
    let mut ofertas = vec![
        oferta_sql(&e.m, &e.emp, "draft", false, "").await,
        oferta_sql(&e.m, &e.emp, "paused", false, "").await,
        oferta_sql(&e.m, &e.emp, "closed", false, "1 day").await,
        oferta_sql(&e.m, &sin_verificar, "published", false, "").await,
        oferta_sql(&e.m, &vencida, "published", false, "").await,
        oferta_sql(&e.m, &inactiva, "published", false, "").await,
        oferta_sql(&e.m, &borrada, "published", false, "").await,
    ];
    ofertas.push(ID_NO.to_string());
    ofertas.push("no-es-uuid".to_string());
    let mut cuerpos = Vec::new();
    for o in &ofertas {
        let r = aplicar(&e, &e.ana, o, aplicar_cuerpo(&MINIMO)).await;
        assert_eq!(
            (r.status, codigo(&r)),
            (StatusCode::NOT_FOUND, "no_encontrado"),
            "{o}"
        );
        for nombre in [
            "cafenuevo",
            "sinverificar",
            "vencida",
            "inactiva",
            "borrada",
        ] {
            assert!(
                !r.text.contains(nombre),
                "el error nombra a la empresa: {}",
                r.text
            );
        }
        cuerpos.push(r.text);
    }
    assert!(cuerpos.windows(2).all(|w| w[0] == w[1]), "los 404 difieren");
    assert_eq!(e.m.n("SELECT count(*) FROM job_applications").await, 0);
    e.m.db.cleanup().await;
}

/// AP5: si se cierra entre que abrió la pantalla y envió, el 404 es el mismo.
#[tokio::test]
async fn oferta_cerrada_entre_medias_404() {
    let e = escenario().await;
    e.m.db
        .exec(&format!(
            "UPDATE job_offers SET status = 'closed', closed_at = now() WHERE id = '{}'",
            e.oferta
        ))
        .await
        .unwrap();
    let r = aplicar(&e, &e.ana, &e.oferta, aplicar_cuerpo(&MINIMO)).await;
    assert_eq!(
        (r.status, codigo(&r)),
        (StatusCode::NOT_FOUND, "no_encontrado")
    );
    e.m.db.cleanup().await;
}

/// AP6: un reclutador no aplica a la oferta de su propia empresa; a otra sí.
#[tokio::test]
async fn oferta_propia_409_y_ajena_si() {
    let e = escenario().await;
    let r = aplicar(&e, &e.rec, &e.oferta, aplicar_cuerpo(&MINIMO)).await;
    assert_eq!(
        (r.status, codigo(&r)),
        (StatusCode::CONFLICT, "oferta_propia")
    );
    assert_eq!(e.m.n("SELECT count(*) FROM job_applications").await, 0);
    let otra = empresa_sql(&e.m, "otra", true, true).await;
    let o2 = oferta_sql(&e.m, &otra, "published", false, "").await;
    let r = aplicar(&e, &e.rec, &o2, aplicar_cuerpo(&MINIMO)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ una sola vez

/// AP3: una sola vez por oferta, para siempre; incluso retirada o rechazada.
#[tokio::test]
async fn ya_aplicaste_incluso_tras_retirar_o_ser_rechazada() {
    let e = escenario().await;
    let v = aplicar_ok(&e, &e.ana, &e.oferta, &MINIMO).await;
    let id = v["id"].as_str().unwrap();
    let r = aplicar(&e, &e.ana, &e.oferta, aplicar_cuerpo(&MINIMO)).await;
    assert_eq!(
        (r.status, codigo(&r)),
        (StatusCode::CONFLICT, "ya_aplicaste")
    );
    assert_eq!(r.json["application_id"], id);
    let w =
        e.m.enviar(
            "POST",
            &format!("/api/v1/me/applications/{id}/withdraw"),
            Some(&e.ana.token),
            None,
        )
        .await;
    assert_eq!(w.status, StatusCode::NO_CONTENT);
    let r = aplicar(&e, &e.ana, &e.oferta, aplicar_cuerpo(&MINIMO)).await;
    assert_eq!(
        (r.status, codigo(&r)),
        (StatusCode::CONFLICT, "ya_aplicaste")
    );
    assert_eq!(e.m.n("SELECT count(*) FROM job_applications").await, 1);

    // Rechazada: también.
    let v2 = aplicar_ok(&e, &e.beto, &e.oferta, &MINIMO).await;
    let id2 = v2["id"].as_str().unwrap();
    mover(&e, id2, Status::UnderReview).await.unwrap();
    mover(&e, id2, Status::Rejected).await.unwrap();
    let r = aplicar(&e, &e.beto, &e.oferta, aplicar_cuerpo(&MINIMO)).await;
    assert_eq!(
        (r.status, codigo(&r)),
        (StatusCode::CONFLICT, "ya_aplicaste")
    );
    e.m.db.cleanup().await;
}

/// AP4: dos simultáneas dejan una.
#[tokio::test]
async fn dos_simultaneas_dejan_una() {
    let e = escenario().await;
    let (a, b) = tokio::join!(
        aplicar(&e, &e.ana, &e.oferta, aplicar_cuerpo(&MINIMO)),
        aplicar(&e, &e.ana, &e.oferta, aplicar_cuerpo(&MINIMO)),
    );
    let mut estados = [a.status, b.status];
    estados.sort();
    assert_eq!(
        estados,
        [StatusCode::CREATED, StatusCode::CONFLICT],
        "{} / {}",
        a.text,
        b.text
    );
    assert_eq!(e.m.n("SELECT count(*) FROM job_applications").await, 1);
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ límite diario

async fn llenar_cupo(e: &Esc, p: &Persona, n: usize, hace_horas: i32) {
    for i in 0..n {
        let o = oferta_sql(&e.m, &e.emp, "published", false, "").await;
        e.m.db
            .exec(&format!(
                "INSERT INTO job_applications (id, applicant_id, job_offer_id, status, shared_fields, \
                    applied_at, status_changed_at) \
                 VALUES (gen_random_uuid(), (SELECT id FROM applicants WHERE user_id = '{}'), '{o}', \
                    '{}', '[\"display_name\",\"skills\",\"experience\"]', \
                    now() - interval '{hace_horas} hours' - interval '{i} seconds', now())",
                p.id,
                if i % 2 == 0 { "applied" } else { "withdrawn" }
            ))
            .await
            .unwrap();
    }
}

/// SX5: 20 en 24 h (cuentan las retiradas); la 21 → 429 con `Retry-After`.
#[tokio::test]
async fn veinte_por_dia_y_la_21_es_429() {
    let e = escenario().await;
    llenar_cupo(&e, &e.ana, 20, 1).await;
    let r = aplicar(&e, &e.ana, &e.oferta, aplicar_cuerpo(&MINIMO)).await;
    assert_eq!(
        (r.status, codigo(&r)),
        (StatusCode::TOO_MANY_REQUESTS, "demasiados_intentos")
    );
    let espera: i64 = r
        .headers
        .get("retry-after")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=86_400).contains(&espera), "{espera}");
    assert_eq!(e.m.n("SELECT count(*) FROM job_applications").await, 20);
    // Otra persona no se ve afectada.
    let r = aplicar(&e, &e.beto, &e.oferta, aplicar_cuerpo(&MINIMO)).await;
    assert_eq!(r.status, StatusCode::CREATED);
    e.m.db.cleanup().await;
}

/// SX5: la ventana es móvil — las de hace más de 24 h no cuentan.
#[tokio::test]
async fn las_de_hace_mas_de_24_horas_no_cuentan() {
    let e = escenario().await;
    llenar_cupo(&e, &e.ana, 20, 25).await;
    let r = aplicar(&e, &e.ana, &e.oferta, aplicar_cuerpo(&MINIMO)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    e.m.db.cleanup().await;
}

/// SX5: lo rechazado por 409 (ya aplicaste, oferta propia) no cuenta y tiene precedencia.
#[tokio::test]
async fn los_409_no_cuentan_y_ganan_al_429() {
    let e = escenario().await;
    aplicar_ok(&e, &e.ana, &e.oferta, &MINIMO).await;
    llenar_cupo(&e, &e.ana, 19, 1).await; // 20 en total
    let r = aplicar(&e, &e.ana, &e.oferta, aplicar_cuerpo(&MINIMO)).await;
    assert_eq!(codigo(&r), "ya_aplicaste", "el 409 va antes que el 429");
    assert_eq!(e.m.n("SELECT count(*) FROM job_applications").await, 20);
    e.m.db.cleanup().await;
}

/// SX5: 25 peticiones simultáneas con 18 previas dejan exactamente 20.
#[tokio::test]
async fn el_limite_aguanta_la_concurrencia() {
    let e = escenario().await;
    llenar_cupo(&e, &e.ana, 18, 1).await;
    let mut ofertas = Vec::new();
    for _ in 0..6 {
        ofertas.push(oferta_sql(&e.m, &e.emp, "published", false, "").await);
    }
    let rs = futures_util::future::join_all(
        ofertas
            .iter()
            .map(|o| aplicar(&e, &e.ana, o, aplicar_cuerpo(&MINIMO))),
    )
    .await;
    let creadas = rs
        .iter()
        .filter(|r| r.status == StatusCode::CREATED)
        .count();
    assert_eq!(
        creadas,
        2,
        "{:?}",
        rs.iter().map(|r| r.status).collect::<Vec<_>>()
    );
    assert_eq!(e.m.n("SELECT count(*) FROM job_applications").await, 20);
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ lo compartido

/// HU-52: la vista compartida lleva SOLO lo consentido; lo demás no existe (ni como null).
#[tokio::test]
async fn lo_compartido_es_solo_lo_consentido() {
    let e = escenario().await;
    let v = aplicar_ok(&e, &e.ana, &e.oferta, &MINIMO).await;
    let id = v["id"].as_str().unwrap();
    let vista = shared_view(&e.m.db.conn, &pii(), id, ahora_micros(&e.m).await)
        .await
        .unwrap()
        .expect("disponible");
    let mut claves: Vec<&str> = vista
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    claves.sort();
    assert_eq!(claves, ["display_name", "experience", "skills"]);
    assert_eq!(vista["display_name"], "Ana P.");
    assert_eq!(vista["skills"][0]["level"], "advanced");
    assert_eq!(vista["skills"][0]["years_of_experience"], 3);
    assert!(vista["skills"][0]["skill"]["code"].is_string());
    let exp = &vista["experience"][0];
    assert_eq!(exp["company"], "Banco X");
    assert_eq!(exp["title"], "Analista");
    assert_eq!(exp["start_date"], "2022-01-01");
    assert!(exp["end_date"].is_null());
    assert!(
        exp.get("description").is_none(),
        "la descripción es un campo aparte"
    );
    let texto = vista.to_string();
    for s in [
        SECRETO_TEL,
        SECRETO_DESC,
        SECRETO_PERFIL,
        SECRETO_CASADA,
        SECRETO_APELLIDO2,
        "SECRETO-MEDIO",
        "correo-prueba",
    ] {
        assert!(!texto.contains(s), "se coló {s}");
    }
    e.m.db.cleanup().await;
}

/// Cada campo opcional aparece si y solo si se marcó.
#[tokio::test]
async fn cada_campo_opcional_va_solo_si_se_marco() {
    let e = escenario().await;
    // (nombre en shared_fields, clave que aparece, texto que debe verse)
    let casos: [(&str, &str, &str); 6] = [
        ("experience_descriptions", "experience", SECRETO_DESC),
        ("languages", "languages", "advanced"),
        ("education", "education", "profession"),
        ("profile_description", "profile_description", SECRETO_PERFIL),
        ("email", "email", "@correo-prueba.com"),
        ("phone", "phone", SECRETO_TEL),
    ];
    for (i, (campo, clave, esperado)) in casos.iter().enumerate() {
        let persona = if i == 0 {
            e.ana.clone()
        } else {
            e.m.persona(&format!("x{i}")).await
        };
        if i != 0 {
            perfil_sql(&e.m, &persona).await;
        }
        let oferta = oferta_sql(&e.m, &e.emp, "published", false, "").await;
        let mut campos = MINIMO.to_vec();
        campos.push(campo);
        let v = aplicar_ok(&e, &persona, &oferta, &campos).await;
        let vista = shared_view(
            &e.m.db.conn,
            &pii(),
            v["id"].as_str().unwrap(),
            ahora_micros(&e.m).await,
        )
        .await
        .unwrap()
        .unwrap();
        let mut claves: Vec<&str> = vista
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        claves.sort();
        let mut esperadas: Vec<&str> = MINIMO.to_vec();
        if *clave != "experience" {
            esperadas.push(clave);
        }
        esperadas.sort();
        assert_eq!(claves, esperadas, "{campo}");
        let texto = vista.to_string();
        assert!(
            texto.contains(esperado),
            "{campo}: falta {esperado} en {texto}"
        );
        // La descripción de la experiencia solo con su propio nombre.
        assert_eq!(
            texto.contains(SECRETO_DESC),
            *campo == "experience_descriptions",
            "{campo}"
        );
    }
    e.m.db.cleanup().await;
}

/// Con todo marcado, jamás salen CV, dirección, documento, nacimiento, género, estado civil,
/// segundo apellido, apellido de casada, foto, transcripciones, plan ni ids de la persona.
#[tokio::test]
async fn con_todo_marcado_jamas_sale_lo_prohibido() {
    let e = escenario().await;
    let todo = [
        "display_name",
        "skills",
        "experience",
        "experience_descriptions",
        "languages",
        "education",
        "profile_description",
        "email",
        "phone",
    ];
    let v = aplicar_ok(&e, &e.ana, &e.oferta, &todo).await;
    let vista = shared_view(
        &e.m.db.conn,
        &pii(),
        v["id"].as_str().unwrap(),
        ahora_micros(&e.m).await,
    )
    .await
    .unwrap()
    .unwrap();
    let texto = vista.to_string();
    for s in [
        SECRETO_CASADA,
        SECRETO_APELLIDO2,
        "SECRETO-MEDIO",
        "storage_key",
        "birth",
        "address",
        "gender",
        "marital",
        "document",
        "photo",
        "transcript",
        "score",
        "plan",
        "cv",
    ] {
        assert!(
            !texto.to_lowercase().contains(&s.to_lowercase()),
            "se coló {s}: {texto}"
        );
    }
    for id_prohibido in [
        e.ana.id.clone(),
        e.m.texto(&format!(
            "SELECT id::text FROM applicants WHERE user_id = '{}'",
            e.ana.id
        ))
        .await,
    ] {
        assert!(
            !texto.contains(&id_prohibido),
            "salió un id permanente de la persona"
        );
    }
    assert_eq!(vista["email"], e.ana.email);
    assert_eq!(vista["phone"], SECRETO_TEL);
    assert_eq!(vista["profile_description"], SECRETO_PERFIL);
    assert_eq!(vista["experience"][0]["description"], SECRETO_DESC);
    e.m.db.cleanup().await;
}

/// AP8 / en vivo: lo compartido se lee del perfil actual; si lo cambia, se ve lo nuevo.
#[tokio::test]
async fn lo_compartido_se_ve_en_vivo() {
    let e = escenario().await;
    let v = aplicar_ok(
        &e,
        &e.ana,
        &e.oferta,
        &["display_name", "skills", "experience", "phone"],
    )
    .await;
    let id = v["id"].as_str().unwrap();
    let nuevo = common::hex(&pii().encrypt("+502 1111 2222").unwrap());
    e.m.db
        .exec(&format!(
            "UPDATE personal_data SET phone = '\\x{nuevo}'::bytea WHERE applicant_id = \
             (SELECT id FROM applicants WHERE user_id = '{}')",
            e.ana.id
        ))
        .await
        .unwrap();
    let vista = shared_view(&e.m.db.conn, &pii(), id, ahora_micros(&e.m).await)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vista["phone"], "+502 1111 2222");
    // Y si lo borra, deja de verse.
    e.m.db
        .exec("UPDATE personal_data SET phone = NULL")
        .await
        .unwrap();
    let vista = shared_view(&e.m.db.conn, &pii(), id, ahora_micros(&e.m).await)
        .await
        .unwrap()
        .unwrap();
    assert!(vista["phone"].is_null());
    e.m.db.cleanup().await;
}

/// La lista compartida es inmutable: no hay ruta para ampliarla.
#[tokio::test]
async fn no_hay_ruta_para_ampliar_lo_compartido() {
    let e = escenario().await;
    let v = aplicar_ok(&e, &e.ana, &e.oferta, &MINIMO).await;
    let id = v["id"].as_str().unwrap();
    for (metodo, ruta) in [
        ("PUT", format!("/api/v1/me/applications/{id}")),
        ("PATCH", format!("/api/v1/me/applications/{id}")),
        ("POST", format!("/api/v1/me/applications/{id}/share")),
    ] {
        let r =
            e.m.enviar(
                metodo,
                &ruta,
                Some(&e.ana.token),
                Some(json!({"shared_fields": ["email"]})),
            )
            .await;
        assert!(
            matches!(
                r.status,
                StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
            ),
            "{metodo} {ruta}: {}",
            r.status
        );
    }
    assert_eq!(
        e.m.texto("SELECT shared_fields::text FROM job_applications")
            .await,
        r#"["display_name", "skills", "experience"]"#
    );
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ retirar, borrar, retención

/// AP8: al retirar, lo compartido deja de verse; el historial queda.
#[tokio::test]
async fn al_retirar_deja_de_verse_lo_compartido() {
    let e = escenario().await;
    let v = aplicar_ok(&e, &e.ana, &e.oferta, &MINIMO).await;
    let id = v["id"].as_str().unwrap();
    assert!(
        shared_view(&e.m.db.conn, &pii(), id, ahora_micros(&e.m).await)
            .await
            .unwrap()
            .is_some()
    );
    let w =
        e.m.enviar(
            "POST",
            &format!("/api/v1/me/applications/{id}/withdraw"),
            Some(&e.ana.token),
            None,
        )
        .await;
    assert_eq!(w.status, StatusCode::NO_CONTENT);
    assert!(
        shared_view(&e.m.db.conn, &pii(), id, ahora_micros(&e.m).await)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        e.m.n("SELECT count(*) FROM job_application_events").await,
        2
    );
    e.m.db.cleanup().await;
}

/// Retirar es idempotente y no escribe un segundo evento; no se puede desde hired/rejected.
#[tokio::test]
async fn retirar_idempotente_y_no_desde_finales() {
    let e = escenario().await;
    let v = aplicar_ok(&e, &e.ana, &e.oferta, &MINIMO).await;
    let id = v["id"].as_str().unwrap();
    let ruta = format!("/api/v1/me/applications/{id}/withdraw");
    for _ in 0..2 {
        let w = e.m.enviar("POST", &ruta, Some(&e.ana.token), None).await;
        assert_eq!(w.status, StatusCode::NO_CONTENT);
    }
    assert_eq!(
        e.m.n("SELECT count(*) FROM job_application_events WHERE to_status = 'withdrawn'")
            .await,
        1
    );
    assert_eq!(
        e.m.n(&format!(
            "SELECT count(*) FROM job_application_events WHERE to_status = 'withdrawn' AND changed_by = '{}'",
            e.ana.id
        ))
        .await,
        1
    );

    for (final_, tag) in [(Status::Hired, "h"), (Status::Rejected, "r")] {
        let p = e.m.persona(tag).await;
        let o = oferta_sql(&e.m, &e.emp, "published", false, "").await;
        let a = aplicar_ok(&e, &p, &o, &MINIMO).await;
        let aid = a["id"].as_str().unwrap();
        mover(&e, aid, Status::UnderReview).await.unwrap();
        if final_ == Status::Hired {
            mover(&e, aid, Status::Shortlisted).await.unwrap();
        }
        mover(&e, aid, final_).await.unwrap();
        let w =
            e.m.enviar(
                "POST",
                &format!("/api/v1/me/applications/{aid}/withdraw"),
                Some(&p.token),
                None,
            )
            .await;
        assert_eq!(
            (w.status, codigo(&w)),
            (StatusCode::CONFLICT, "estado_invalido")
        );
        assert_eq!(
            w.json["status"],
            if final_ == Status::Hired {
                "hired"
            } else {
                "rejected"
            }
        );
    }
    e.m.db.cleanup().await;
}

/// SX2/SX6: una aplicación ajena, inexistente o mal formada → 404 idéntico (leer y retirar).
#[tokio::test]
async fn la_aplicacion_ajena_es_404_para_leer_y_retirar() {
    let e = escenario().await;
    let v = aplicar_ok(&e, &e.ana, &e.oferta, &MINIMO).await;
    let id = v["id"].as_str().unwrap();
    let mut cuerpos = Vec::new();
    for ruta in [
        format!("/api/v1/me/applications/{id}"),
        format!("/api/v1/me/applications/{ID_NO}"),
        "/api/v1/me/applications/no-es-uuid".to_string(),
    ] {
        let r = e.m.enviar("GET", &ruta, Some(&e.beto.token), None).await;
        assert_eq!(
            (r.status, codigo(&r)),
            (StatusCode::NOT_FOUND, "no_encontrado"),
            "{ruta}"
        );
        cuerpos.push(r.text);
        let w =
            e.m.enviar(
                "POST",
                &format!("{ruta}/withdraw"),
                Some(&e.beto.token),
                None,
            )
            .await;
        assert_eq!(w.status, StatusCode::NOT_FOUND, "{ruta}/withdraw");
    }
    assert!(cuerpos.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(estado_de(&e, id).await, "applied", "lo ajeno no se tocó");
    // Y la suya sí se lee.
    let r =
        e.m.get(&format!("/api/v1/me/applications/{id}"), &e.ana)
            .await;
    assert_eq!(r.status, StatusCode::OK);
    e.m.db.cleanup().await;
}

/// BO1, BO2, PA10: borrar la cuenta deja la aplicación anónima, sin `shared_fields`, y borra
/// los registros externos. El `changed_by` de la persona pasa a null.
#[tokio::test]
async fn borrar_la_cuenta_deja_la_aplicacion_anonima() {
    let e = escenario().await;
    let v = aplicar_ok(
        &e,
        &e.ana,
        &e.oferta,
        &["display_name", "skills", "experience", "email"],
    )
    .await;
    let id = v["id"].as_str().unwrap();
    e.m.enviar(
        "POST",
        "/api/v1/me/external-applications",
        Some(&e.ana.token),
        Some(json!({"company_name": "Banco X", "position": "Analista", "applied_on": "2026-11-05", "status": "applied"})),
    )
    .await;
    assert_eq!(e.m.n("SELECT count(*) FROM external_applications").await, 1);
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
        e.m.n("SELECT count(*) FROM job_applications WHERE applicant_id IS NULL")
            .await,
        1
    );
    assert_eq!(
        e.m.texto("SELECT shared_fields::text FROM job_applications")
            .await,
        "[]"
    );
    assert_eq!(
        e.m.n("SELECT count(*) FROM job_application_events WHERE changed_by IS NULL")
            .await,
        1
    );
    assert_eq!(e.m.n("SELECT count(*) FROM external_applications").await, 0);
    assert!(
        shared_view(&e.m.db.conn, &pii(), id, ahora_micros(&e.m).await)
            .await
            .unwrap()
            .is_none()
    );
    // La oferta y la empresa quedan.
    assert_eq!(e.m.n("SELECT count(*) FROM job_offers").await, 1);
    e.m.db.cleanup().await;
}

/// Decisión 5: a los 90 días de `closed_at` ya no se ve lo compartido; un instante antes, sí.
#[tokio::test]
async fn retencion_de_90_dias_desde_el_cierre() {
    let e = escenario().await;
    let v = aplicar_ok(&e, &e.ana, &e.oferta, &MINIMO).await;
    let id = v["id"].as_str().unwrap();
    e.m.db
        .exec(&format!(
            "UPDATE job_offers SET status = 'closed', closed_at = timestamptz '2026-01-01 00:00:00+00' WHERE id = '{}'",
            e.oferta
        ))
        .await
        .unwrap();
    let cierre: i64 = e.m
        .texto("SELECT ((extract(epoch from timestamptz '2026-01-01 00:00:00+00') * 1000000)::bigint)::text")
        .await
        .parse()
        .unwrap();
    let dia = 86_400 * 1_000_000;
    let limite = cierre + 90 * dia;
    assert!(shared_view(&e.m.db.conn, &pii(), id, limite - 1)
        .await
        .unwrap()
        .is_some());
    assert!(shared_view(&e.m.db.conn, &pii(), id, limite)
        .await
        .unwrap()
        .is_none());
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ leer

/// «Mis aplicaciones»: solo las mías, más nuevas primero, con cursor; el resumen no trae
/// `history` ni `shared_fields`.
#[tokio::test]
async fn mis_aplicaciones_solo_las_mias_paginadas() {
    let e = escenario().await;
    let mut ids = Vec::new();
    for _ in 0..5 {
        let o = oferta_sql(&e.m, &e.emp, "published", false, "").await;
        ids.push(
            aplicar_ok(&e, &e.ana, &o, &MINIMO).await["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    aplicar_ok(&e, &e.beto, &e.oferta, &MINIMO).await;
    let mut vistos = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let q = match &cursor {
            Some(c) => format!("?limit=2&cursor={c}"),
            None => "?limit=2".to_string(),
        };
        let r =
            e.m.get(&format!("/api/v1/me/applications{q}"), &e.ana)
                .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
        for i in r.json["items"].as_array().unwrap() {
            assert!(
                i.get("history").is_none() && i.get("shared_fields").is_none(),
                "{i}"
            );
            assert_eq!(i["status"], "applied");
            vistos.push(i["id"].as_str().unwrap().to_string());
        }
        match r.json["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    let mut esperado = ids.clone();
    esperado.reverse();
    assert_eq!(vistos, esperado, "más nuevas primero y solo las mías");
    e.m.db.cleanup().await;
}

#[tokio::test]
async fn mis_aplicaciones_filtra_por_estado_y_cierra_los_parametros() {
    let e = escenario().await;
    let v = aplicar_ok(&e, &e.ana, &e.oferta, &MINIMO).await;
    let id = v["id"].as_str().unwrap();
    let o2 = oferta_sql(&e.m, &e.emp, "published", false, "").await;
    aplicar_ok(&e, &e.ana, &o2, &MINIMO).await;
    mover(&e, id, Status::UnderReview).await.unwrap();
    let r =
        e.m.get("/api/v1/me/applications?status=under_review", &e.ana)
            .await;
    assert_eq!(r.json["items"].as_array().unwrap().len(), 1);
    let r =
        e.m.get("/api/v1/me/applications?status=hired", &e.ana)
            .await;
    assert_eq!(r.json["items"].as_array().unwrap().len(), 0);
    for q in ["?status=nope", "?limit=0", "?limit=51", "?cursor=zzz"] {
        let r =
            e.m.get(&format!("/api/v1/me/applications{q}"), &e.ana)
                .await;
        assert_eq!(
            (r.status, codigo(&r)),
            (StatusCode::UNPROCESSABLE_ENTITY, "datos_invalidos"),
            "{q}"
        );
    }
    for q in ["?age=77", "?sort=zzz", "?q=qqq"] {
        let r =
            e.m.get(&format!("/api/v1/me/applications{q}"), &e.ana)
                .await;
        assert_eq!(
            (r.status, codigo(&r)),
            (StatusCode::UNPROCESSABLE_ENTITY, "parametro_no_permitido"),
            "{q}"
        );
        for valor in ["age", "77", "zzz", "qqq"] {
            assert!(
                !r.text.contains(&format!("{valor}=")) && !r.text.contains("77"),
                "repite lo recibido"
            );
        }
    }
    e.m.db.cleanup().await;
}

/// AP7: la persona ve `history` = [{status, at}], jamás `changed_by` ni `note`.
#[tokio::test]
async fn el_historial_de_la_persona_no_trae_nota_ni_quien_decidio() {
    let e = escenario().await;
    let v = aplicar_ok(&e, &e.ana, &e.oferta, &MINIMO).await;
    let id = v["id"].as_str().unwrap();
    move_status(
        &e.m.db.conn,
        &pii(),
        Scope::Company {
            company_id: &e.emp,
            user_id: &e.rec.id,
        },
        id,
        Status::UnderReview,
        Some("NOTA-INTERNA-SECRETA"),
    )
    .await
    .unwrap();
    let r =
        e.m.get(&format!("/api/v1/me/applications/{id}"), &e.ana)
            .await;
    assert_eq!(r.status, StatusCode::OK);
    let h = r.json["history"].as_array().unwrap();
    assert_eq!(h.len(), 2);
    assert_eq!(h[0]["status"], "applied");
    assert_eq!(h[1]["status"], "under_review");
    for ev in h {
        let mut k: Vec<&str> = ev.as_object().unwrap().keys().map(String::as_str).collect();
        k.sort();
        assert_eq!(k, ["at", "status"]);
    }
    assert!(!r.text.contains("NOTA-INTERNA") && !r.text.contains(&e.rec.id));
    assert_eq!(r.json["status"], "under_review");
    // La nota se guarda cifrada.
    let bytes = common::bytes_de(
        &e.m.db,
        "SELECT note FROM job_application_events WHERE note IS NOT NULL",
    )
    .await;
    assert!(!common::contiene(&bytes, b"NOTA-INTERNA"));
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ máquina de estados

/// PA5: hired solo desde shortlisted; nada reabre; el reclutador de otra empresa no existe.
#[tokio::test]
async fn move_status_respeta_la_maquina_y_el_alcance() {
    let e = escenario().await;
    let v = aplicar_ok(&e, &e.ana, &e.oferta, &MINIMO).await;
    let id = v["id"].as_str().unwrap();
    // hired desde applied: inválido, y no toca nada.
    match mover(&e, id, Status::Hired).await {
        Err(MoveError::Invalid(Status::Applied)) => {}
        r => panic!("{r:?}"),
    }
    assert!(matches!(
        mover(&e, id, Status::Shortlisted).await,
        Err(MoveError::Invalid(Status::Applied))
    ));
    assert!(matches!(
        mover(&e, id, Status::Rejected).await,
        Err(MoveError::Invalid(Status::Applied))
    ));
    assert!(matches!(
        mover(&e, id, Status::Withdrawn).await,
        Err(MoveError::Invalid(_))
    ));
    assert_eq!(estado_de(&e, id).await, "applied");
    assert_eq!(
        e.m.n("SELECT count(*) FROM job_application_events").await,
        1
    );
    mover(&e, id, Status::UnderReview).await.unwrap();
    assert!(matches!(
        mover(&e, id, Status::UnderReview).await,
        Err(MoveError::Invalid(Status::UnderReview))
    ));
    assert!(matches!(
        mover(&e, id, Status::Hired).await,
        Err(MoveError::Invalid(Status::UnderReview))
    ));
    mover(&e, id, Status::Shortlisted).await.unwrap();
    mover(&e, id, Status::Hired).await.unwrap();
    assert!(matches!(
        mover(&e, id, Status::Rejected).await,
        Err(MoveError::Invalid(Status::Hired))
    ));
    assert_eq!(
        e.m.n("SELECT count(*) FROM job_application_events").await,
        4
    );

    // Otra empresa: NotFound, sin tocar.
    let otra = empresa_sql(&e.m, "otra", true, true).await;
    let ajeno = reclutador_sql(&e.m, "ajeno", &otra).await;
    let p = e.m.persona("carla").await;
    let o = oferta_sql(&e.m, &e.emp, "published", false, "").await;
    let a = aplicar_ok(&e, &p, &o, &MINIMO).await;
    let aid = a["id"].as_str().unwrap();
    let r = move_status(
        &e.m.db.conn,
        &pii(),
        Scope::Company {
            company_id: &otra,
            user_id: &ajeno.id,
        },
        aid,
        Status::UnderReview,
        None,
    )
    .await;
    assert!(matches!(r, Err(MoveError::NotFound)), "{r:?}");
    // La persona ajena tampoco.
    let r = move_status(
        &e.m.db.conn,
        &pii(),
        Scope::Applicant {
            user_id: &e.beto.id,
        },
        aid,
        Status::Withdrawn,
        None,
    )
    .await;
    assert!(matches!(r, Err(MoveError::NotFound)));
    // Id mal formado.
    let r = move_status(
        &e.m.db.conn,
        &pii(),
        Scope::Applicant { user_id: &p.id },
        "no-uuid",
        Status::Withdrawn,
        None,
    )
    .await;
    assert!(matches!(r, Err(MoveError::NotFound)));
    assert_eq!(estado_de(&e, aid).await, "applied");
    e.m.db.cleanup().await;
}

/// Dos movimientos simultáneos dejan uno: el segundo ve el estado nuevo.
#[tokio::test]
async fn dos_movimientos_simultaneos_dejan_uno() {
    let e = escenario().await;
    let v = aplicar_ok(&e, &e.ana, &e.oferta, &MINIMO).await;
    let id = v["id"].as_str().unwrap();
    mover(&e, id, Status::UnderReview).await.unwrap();
    let (a, b) = tokio::join!(
        mover(&e, id, Status::Rejected),
        mover(&e, id, Status::Rejected)
    );
    assert_eq!(a.is_ok() as u8 + b.is_ok() as u8, 1, "{a:?} / {b:?}");
    assert_eq!(
        e.m.n("SELECT count(*) FROM job_application_events").await,
        3
    );
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ confidenciales

/// HU-55: la empresa se revela, por aplicación, al pasar a `shortlisted`, y no se esconde después.
#[tokio::test]
async fn la_empresa_confidencial_se_revela_solo_al_shortlisted_y_por_aplicacion() {
    let e = escenario().await;
    let conf = oferta_sql(&e.m, &e.emp, "published", true, "").await;
    let a1 = aplicar_ok(&e, &e.ana, &conf, &MINIMO).await;
    let a2 = aplicar_ok(&e, &e.beto, &conf, &MINIMO).await;
    assert!(a1["offer"]["company"].is_null());
    assert_eq!(a1["offer"]["confidential"], true);
    let id1 = a1["id"].as_str().unwrap();
    let id2 = a2["id"].as_str().unwrap();
    let company_de = |r: &Resp| r.json["offer"]["company"].clone();
    mover(&e, id1, Status::UnderReview).await.unwrap();
    let r =
        e.m.get(&format!("/api/v1/me/applications/{id1}"), &e.ana)
            .await;
    assert!(company_de(&r).is_null(), "no se revela en under_review");
    mover(&e, id1, Status::Shortlisted).await.unwrap();
    let r =
        e.m.get(&format!("/api/v1/me/applications/{id1}"), &e.ana)
            .await;
    assert_eq!(company_de(&r)["name"], "cafenuevo");
    assert!(company_de(&r)["id"].is_string() && company_de(&r).get("website").is_some());
    // Por aplicación: la otra persona sigue sin verla.
    let r2 =
        e.m.get(&format!("/api/v1/me/applications/{id2}"), &e.beto)
            .await;
    assert!(company_de(&r2).is_null());
    let lista = e.m.get("/api/v1/me/applications", &e.beto).await;
    assert!(lista.json["items"][0]["offer"]["company"].is_null());
    // Rechazada después: sigue revelada.
    mover(&e, id1, Status::Rejected).await.unwrap();
    let r =
        e.m.get(&format!("/api/v1/me/applications/{id1}"), &e.ana)
            .await;
    assert_eq!(company_de(&r)["name"], "cafenuevo");
    // Rechazada sin pasar por shortlisted: nunca.
    mover(&e, id2, Status::UnderReview).await.unwrap();
    mover(&e, id2, Status::Rejected).await.unwrap();
    let r2 =
        e.m.get(&format!("/api/v1/me/applications/{id2}"), &e.beto)
            .await;
    assert!(company_de(&r2).is_null());
    assert!(!r2.text.contains("cafenuevo"));
    e.m.db.cleanup().await;
}

/// La confidencial no nombra a la empresa en ningún error de aplicar.
#[tokio::test]
async fn ningun_error_de_una_confidencial_nombra_la_empresa() {
    let e = escenario().await;
    let conf = oferta_sql(&e.m, &e.emp, "published", true, "").await;
    aplicar_ok(&e, &e.ana, &conf, &MINIMO).await;
    let r = aplicar(&e, &e.ana, &conf, aplicar_cuerpo(&MINIMO)).await;
    assert_eq!(codigo(&r), "ya_aplicaste");
    assert!(!r.text.contains("cafenuevo"));
    let r = aplicar(&e, &e.rec, &conf, aplicar_cuerpo(&MINIMO)).await;
    assert_eq!(codigo(&r), "oferta_propia");
    assert!(!r.text.contains("cafenuevo"));
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ conteo real

/// OF7: el contador de aplicaciones sale de `job_applications`.
#[tokio::test]
async fn el_conteo_de_la_oferta_sale_de_las_aplicaciones_reales() {
    let e = escenario().await;
    assert_eq!(e.m.n("SELECT count(*) FROM information_schema.columns WHERE table_name = 'job_offers' AND column_name = 'applications_count'").await, 0);
    aplicar_ok(&e, &e.ana, &e.oferta, &MINIMO).await;
    aplicar_ok(&e, &e.beto, &e.oferta, &MINIMO).await;
    let admin = reclutador_sql(&e.m, "lector", &e.emp).await;
    let r = e.m.get("/api/v1/company/offers", &admin).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let item = r.json["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == e.oferta.as_str())
        .unwrap()
        .clone();
    assert_eq!(item["applications_count"], 2);
    let d =
        e.m.get(&format!("/api/v1/company/offers/{}", e.oferta), &admin)
            .await;
    assert_eq!(d.json["applications_count"], 2);
    e.m.db.cleanup().await;
}

// ------------------------------------------------------------------ registros

/// Nada de lo que la persona comparte llega a un log: ni valores, ni nombres de campo del perfil.
#[tokio::test]
async fn nada_de_lo_compartido_llega_al_log() {
    let (registro, _g) = capturar_log();
    let e = escenario().await;
    let todo = [
        "display_name",
        "skills",
        "experience",
        "experience_descriptions",
        "languages",
        "education",
        "profile_description",
        "email",
        "phone",
    ];
    let v = aplicar_ok(&e, &e.ana, &e.oferta, &todo).await;
    let id = v["id"].as_str().unwrap();
    let _ = aplicar(&e, &e.ana, &e.oferta, aplicar_cuerpo(&todo)).await;
    let _ = aplicar(
        &e,
        &e.ana,
        &e.oferta,
        json!({"consent": false, "shared_fields": ["secreto-ZZ"]}),
    )
    .await;
    let _ =
        e.m.get(&format!("/api/v1/me/applications/{id}"), &e.ana)
            .await;
    let _ = e.m.get("/api/v1/me/applications?age=77", &e.ana).await;
    move_status(
        &e.m.db.conn,
        &pii(),
        Scope::Company {
            company_id: &e.emp,
            user_id: &e.rec.id,
        },
        id,
        Status::UnderReview,
        Some("NOTA-SECRETA-LOG"),
    )
    .await
    .unwrap();
    let _ = shared_view(&e.m.db.conn, &pii(), id, ahora_micros(&e.m).await).await;
    let _ =
        e.m.enviar(
            "POST",
            &format!("/api/v1/me/applications/{id}/withdraw"),
            Some(&e.ana.token),
            None,
        )
        .await;
    let log = registro.texto();
    assert!(log.contains("captura-activa"), "la captura no funciona");
    for s in [
        SECRETO_TEL,
        SECRETO_DESC,
        SECRETO_PERFIL,
        "Banco X",
        "NOTA-SECRETA",
        "secreto-ZZ",
        &e.ana.email,
        "Ana P.",
    ] {
        assert!(!log.contains(s), "el log contiene {s}");
    }
    e.m.db.cleanup().await;
}

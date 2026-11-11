//! Casos de verdad del matching (Fase 5b, paso 8: MA1–MA10 en lo que toca a la API), contra
//! «Matching (Fase 5b)» de `api.md`. PostgreSQL real, una base por prueba. Se escribieron antes
//! que el código, en rojo.
//!
//! Empresas, ofertas y perfiles se insertan por SQL; las aplicaciones, por la ruta de aplicar.
//! Ningún reloj de pared: las fechas salen de `current_date` de la base más un desplazamiento.

mod common;
#[path = "common/empresas.rs"]
mod empresas;

use axum::http::StatusCode;
use empresas::{mundo, Mundo, Persona, Resp};
use serde_json::{json, Value};

const OFERTAS: &str = "/api/v1/offers";

// ------------------------------------------------------------------ fixtures

async fn cat(m: &Mundo, tabla: &str, orden: &str, n: usize) -> String {
    m.texto(&format!(
        "SELECT id::text FROM {tabla} ORDER BY {orden} OFFSET {n} LIMIT 1"
    ))
    .await
}
async fn skill(m: &Mundo, n: usize) -> String {
    cat(m, "skills", "code", n).await
}
async fn idioma(m: &Mundo, n: usize) -> String {
    cat(m, "languages", "code", n).await
}
async fn nivel(m: &Mundo, n: usize) -> String {
    cat(m, "academic_levels", "sort_order", n).await
}

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

/// Una oferta publicada hace `dias` días, sin requisitos todavía.
async fn oferta(m: &Mundo, company: &str, titulo: &str, dias: i32) -> String {
    let id = m.texto("SELECT gen_random_uuid()::text").await;
    m.db.exec(&format!(
        "INSERT INTO job_offers (id, company_id, name, description, status, publication_date) \
         VALUES ('{id}', '{company}', '{titulo}', 'Una oferta.', 'published', current_date - {dias})"
    ))
    .await
    .unwrap();
    id
}

async fn pide_skill(m: &Mundo, oferta: &str, skill: &str, obligatoria: bool, min: Option<&str>) {
    let min = min.map_or("NULL".to_string(), |x| format!("'{x}'"));
    m.db.exec(&format!(
        "INSERT INTO job_offer_skills (job_offer_id, skill_id, is_required, min_level) \
         VALUES ('{oferta}', '{skill}', {obligatoria}, {min})"
    ))
    .await
    .unwrap();
}

async fn pide_idioma(m: &Mundo, oferta: &str, idioma: &str, min: &str) {
    m.db.exec(&format!(
        "INSERT INTO job_offer_languages (job_offer_id, language_id, min_proficiency) \
         VALUES ('{oferta}', '{idioma}', '{min}')"
    ))
    .await
    .unwrap();
}

async fn applicant(m: &Mundo, p: &Persona) -> String {
    m.texto(&format!(
        "SELECT id::text FROM applicants WHERE user_id = '{}'",
        p.id
    ))
    .await
}

async fn tiene_skill(m: &Mundo, p: &Persona, skill: &str, nivel: &str) {
    let a = applicant(m, p).await;
    m.db.exec(&format!(
        "INSERT INTO applicant_skills (id, applicant_id, skill_id, level) \
         VALUES (gen_random_uuid(), '{a}', '{skill}', '{nivel}')"
    ))
    .await
    .unwrap();
}

async fn tiene_idioma(m: &Mundo, p: &Persona, idioma: &str, dominio: &str) {
    let a = applicant(m, p).await;
    m.db.exec(&format!(
        "INSERT INTO applicant_languages (id, applicant_id, language_id, proficiency) \
         VALUES (gen_random_uuid(), '{a}', '{idioma}', '{dominio}')"
    ))
    .await
    .unwrap();
}

async fn tiene_nivel(m: &Mundo, p: &Persona, nivel: &str) {
    let a = applicant(m, p).await;
    m.db.exec(&format!(
        "INSERT INTO professional_data (id, applicant_id, last_academic_level_id) \
         VALUES (gen_random_uuid(), '{a}', '{nivel}')"
    ))
    .await
    .unwrap();
}

/// Experiencia mínima para que la aplicación tenga qué compartir.
async fn tiene_experiencia(m: &Mundo, p: &Persona, desde: &str) {
    let a = applicant(m, p).await;
    m.db.exec(&format!(
        "INSERT INTO applicant_experiences (id, applicant_id, company, title, start_date, position) \
         VALUES (gen_random_uuid(), '{a}', 'Banco X', 'Analista', '{desde}', 0)"
    ))
    .await
    .unwrap();
}

async fn detalle(m: &Mundo, p: &Persona, oferta: &str) -> Value {
    let r = m.get(&format!("{OFERTAS}/{oferta}"), p).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    r.json
}

async fn lista(m: &Mundo, p: &Persona, q: &str) -> Resp {
    m.get(&format!("{OFERTAS}?{q}"), p).await
}

fn ids(r: &Resp) -> Vec<String> {
    r.json["items"]
        .as_array()
        .unwrap_or_else(|| panic!("sin items: {}", r.text))
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect()
}

fn de(xs: [&String; 3]) -> Vec<String> {
    xs.iter().map(|s| (*s).clone()).collect()
}

fn componente<'a>(m: &'a Value, clave: &str) -> &'a Value {
    m["breakdown"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == clave)
        .unwrap_or_else(|| panic!("sin componente {clave}"))
}

/// Tres ofertas (la 1 pide la habilidad 0 obligatoria con nivel intermedio y la 1 opcional; la 2,
/// la habilidad 1 obligatoria; la 3, la 2 obligatoria), publicadas hace 1, 0 y 2 días.
struct Tres {
    o1: String,
    o2: String,
    o3: String,
}

async fn tres_ofertas(m: &Mundo) -> Tres {
    let emp = empresa(m, "cafenuevo").await;
    let (s0, s1, s2) = (skill(m, 0).await, skill(m, 1).await, skill(m, 2).await);
    let o1 = oferta(m, &emp, "Uno", 1).await;
    pide_skill(m, &o1, &s0, true, Some("intermediate")).await;
    pide_skill(m, &o1, &s1, false, None).await;
    let o2 = oferta(m, &emp, "Dos", 0).await;
    pide_skill(m, &o2, &s1, true, None).await;
    let o3 = oferta(m, &emp, "Tres", 2).await;
    pide_skill(m, &o3, &s2, true, None).await;
    Tres { o1, o2, o3 }
}

// ================================================================== lado candidato

/// MA1/MA2: el detalle trae el `match` propio con puntaje y desglose.
#[tokio::test]
async fn ma1_ma2_el_detalle_trae_mi_match_con_su_desglose() {
    let m = mundo().await;
    let t = tres_ofertas(&m).await;
    let ana = m.persona("ana").await;
    tiene_skill(&m, &ana, &skill(&m, 0).await, "advanced").await;

    let d = detalle(&m, &ana, &t.o1).await;
    assert_eq!(d["match"]["score"], 67, "{d}");
    let s = componente(&d["match"], "skills");
    assert_eq!(s["weight"], 50);
    assert_eq!(s["applies"], true);
    assert_eq!(s["matched"].as_array().unwrap().len(), 1);
    assert_eq!(s["missing"].as_array().unwrap().len(), 1);
    assert_eq!(s["matched"][0]["required"], true);
    assert_eq!(s["matched"][0]["skill"]["id"], skill(&m, 0).await);
    assert_eq!(componente(&d["match"], "languages")["applies"], false);
    assert_eq!(d["match"]["breakdown"]["empty_profile"], false);
    assert!(d["match"]["breakdown"]["reason"].is_null());
    // Mismo perfil, misma oferta: mismo resultado (MA1).
    assert_eq!(detalle(&m, &ana, &t.o1).await["match"], d["match"]);
    m.db.cleanup().await;
}

/// El listado trae el `match` de **quien llama**, nunca el de otra persona.
#[tokio::test]
async fn el_listado_trae_el_match_de_quien_llama() {
    let m = mundo().await;
    let t = tres_ofertas(&m).await;
    let ana = m.persona("ana").await;
    let beto = m.persona("beto").await;
    tiene_skill(&m, &ana, &skill(&m, 0).await, "advanced").await;
    tiene_skill(&m, &beto, &skill(&m, 1).await, "beginner").await;

    let puntaje = |r: &Resp, id: &str| -> i64 {
        r.json["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == id)
            .unwrap()["match"]["score"]
            .as_i64()
            .unwrap()
    };
    let la = lista(&m, &ana, "").await;
    let lb = lista(&m, &beto, "").await;
    assert_eq!(la.status, StatusCode::OK, "{}", la.text);
    assert_eq!(
        [&t.o1, &t.o2, &t.o3].map(|o| puntaje(&la, o)),
        [67, 0, 0],
        "ana"
    );
    assert_eq!(
        [&t.o1, &t.o2, &t.o3].map(|o| puntaje(&lb, o)),
        [33, 100, 0],
        "beto"
    );
    assert_eq!(la.json["sorted_by"], "recent");
    assert!(la.json["match_unavailable"].is_null());
    // El detalle coincide con el listado.
    assert_eq!(detalle(&m, &beto, &t.o2).await["match"]["score"], 100);
    m.db.cleanup().await;
}

/// MA6: `sort=match` ordena por mi puntaje, empate por fecha e id, y pagina con cursor propio.
#[tokio::test]
async fn ma6_sort_match_ordena_por_mi_puntaje_y_pagina() {
    let m = mundo().await;
    let t = tres_ofertas(&m).await;
    let ana = m.persona("ana").await;
    let beto = m.persona("beto").await;
    tiene_skill(&m, &ana, &skill(&m, 0).await, "advanced").await;
    tiene_skill(&m, &beto, &skill(&m, 1).await, "beginner").await;

    // Ana: Uno 67, Dos 0 (hoy), Tres 0 (hace 2 días): el empate va por fecha descendente.
    let r = lista(&m, &ana, "sort=match").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(ids(&r), de([&t.o1, &t.o2, &t.o3]));
    assert_eq!(r.json["sorted_by"], "match");
    assert!(r.json["match_unavailable"].is_null());
    // Sin pedir match el orden es otro (por fecha): Dos, Uno, Tres.
    assert_eq!(ids(&lista(&m, &ana, "").await), de([&t.o2, &t.o1, &t.o3]));
    // Beto: Dos 100, Uno 33, Tres 0.
    assert_eq!(
        ids(&lista(&m, &beto, "sort=match").await),
        de([&t.o2, &t.o1, &t.o3])
    );

    // Paginar de a una devuelve lo mismo, sin repetir ni saltar.
    let mut vistos = vec![];
    let mut cursor: Option<String> = None;
    for _ in 0..5 {
        let q = match &cursor {
            Some(c) => format!("sort=match&limit=1&cursor={c}"),
            None => "sort=match&limit=1".to_string(),
        };
        let r = lista(&m, &ana, &q).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
        vistos.extend(ids(&r));
        match r.json["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    assert_eq!(vistos, [t.o1.clone(), t.o2.clone(), t.o3.clone()]);
    m.db.cleanup().await;
}

/// El cursor solo sirve para el orden con el que se emitió (también el de `match`).
#[tokio::test]
async fn el_cursor_de_match_no_sirve_en_otro_orden_ni_al_reves() {
    let m = mundo().await;
    tres_ofertas(&m).await;
    let ana = m.persona("ana").await;
    tiene_skill(&m, &ana, &skill(&m, 0).await, "advanced").await;
    let c_match = lista(&m, &ana, "sort=match&limit=1").await.json["next_cursor"]
        .as_str()
        .unwrap()
        .to_string();
    let c_reciente = lista(&m, &ana, "limit=1").await.json["next_cursor"]
        .as_str()
        .unwrap()
        .to_string();
    for q in [
        format!("limit=1&cursor={c_match}"),
        format!("sort=oldest&cursor={c_match}"),
        format!("sort=match&cursor={c_reciente}"),
        "sort=match&cursor=basura".to_string(),
    ] {
        let r = lista(&m, &ana, &q).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{q}");
        assert_eq!(r.json["error"], "datos_invalidos", "{q}");
    }
    m.db.cleanup().await;
}

/// MA6: sin perfil ordena por fecha y lo dice.
#[tokio::test]
async fn ma6_sin_perfil_ordena_por_fecha_y_lo_dice() {
    let m = mundo().await;
    let t = tres_ofertas(&m).await;
    let cris = m.persona("cris").await;
    let r = lista(&m, &cris, "sort=match").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(ids(&r), de([&t.o2, &t.o1, &t.o3]));
    assert_eq!(r.json["sorted_by"], "recent");
    assert_eq!(r.json["match_unavailable"], "perfil_vacio");
    // MA10: el match de un perfil vacío es bajo y se explica, no es un error.
    let d = detalle(&m, &cris, &t.o1).await;
    assert_eq!(d["match"]["score"], 0);
    assert_eq!(d["match"]["breakdown"]["empty_profile"], true);
    assert_eq!(
        componente(&d["match"], "skills")["missing"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    m.db.cleanup().await;
}

/// `sorted_by` en toda respuesta del listado.
#[tokio::test]
async fn el_listado_siempre_dice_como_se_ordeno() {
    let m = mundo().await;
    tres_ofertas(&m).await;
    let ana = m.persona("ana").await;
    for (q, esperado) in [
        ("", "recent"),
        ("sort=recent", "recent"),
        ("sort=oldest", "oldest"),
    ] {
        assert_eq!(lista(&m, &ana, q).await.json["sorted_by"], esperado, "{q}");
    }
    // Un valor que no existe sigue siendo 422, y otro criterio, parámetro no permitido.
    assert_eq!(
        lista(&m, &ana, "sort=").await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let r = lista(&m, &ana, "sort=age").await;
    assert_eq!(r.json["error"], "parametro_no_permitido");
    m.db.cleanup().await;
}

/// MA7: si cambia mi perfil o la oferta, el puntaje siguiente ya es otro (se calcula al leer).
#[tokio::test]
async fn ma7_el_puntaje_sigue_al_perfil_y_a_la_oferta() {
    let m = mundo().await;
    let t = tres_ofertas(&m).await;
    let ana = m.persona("ana").await;
    tiene_skill(&m, &ana, &skill(&m, 0).await, "advanced").await;
    assert_eq!(detalle(&m, &ana, &t.o2).await["match"]["score"], 0);

    // Perfil: por la ruta del perfil, no por SQL.
    let r = m
        .enviar(
            "PUT",
            "/api/v1/profile",
            Some(&ana.token),
            Some(json!({"skills": [{"skill_id": skill(&m, 1).await, "level": "expert"}]})),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    assert_eq!(detalle(&m, &ana, &t.o2).await["match"]["score"], 100);
    assert_eq!(detalle(&m, &ana, &t.o1).await["match"]["score"], 33);
    assert_eq!(ids(&lista(&m, &ana, "sort=match").await)[0], t.o2);

    // Oferta: agrega un requisito obligatorio que Ana no tiene → baja para todos.
    pide_skill(&m, &t.o2, &skill(&m, 5).await, true, None).await;
    // Ahora pide dos obligatorias (peso 2 + 2) y Ana tiene una: 2 de 4.
    assert_eq!(detalle(&m, &ana, &t.o2).await["match"]["score"], 50);
    m.db.cleanup().await;
}

/// Idiomas, experiencia y nivel académico entran en el puntaje con sus pesos.
#[tokio::test]
async fn el_puntaje_suma_idiomas_experiencia_y_nivel() {
    let m = mundo().await;
    let emp = empresa(&m, "cafenuevo").await;
    let o = oferta(&m, &emp, "Completa", 0).await;
    pide_skill(&m, &o, &skill(&m, 0).await, true, None).await;
    pide_idioma(&m, &o, &idioma(&m, 0).await, "advanced").await;
    m.db.exec(&format!(
        "UPDATE job_offers SET is_experience_required = true, required_experience = 4, \
                minimum_academic_level_id = '{}' WHERE id = '{o}'",
        nivel(&m, 2).await
    ))
    .await
    .unwrap();
    let ana = m.persona("ana").await;
    tiene_skill(&m, &ana, &skill(&m, 0).await, "beginner").await;
    tiene_idioma(&m, &ana, &idioma(&m, 0).await, "basic").await; // por debajo: mitad
    tiene_nivel(&m, &ana, &nivel(&m, 3).await).await; // por encima
                                                      // Dos años de los cuatro, contados hasta hoy: empezó hace 2 años exactos.
    let desde = m
        .texto("SELECT (current_date - interval '2 years')::date::text")
        .await;
    tiene_experiencia(&m, &ana, &desde).await;

    let d = detalle(&m, &ana, &o).await;
    // 50 + 7.5 + ~10 + 15 = ~82.5 de 100.
    let score = d["match"]["score"].as_i64().unwrap();
    assert!((82..=83).contains(&score), "{d}");
    let c = |k| componente(&d["match"], k).clone();
    assert_eq!(c("skills")["points"], 50.0);
    assert_eq!(c("languages")["points"], 7.5);
    assert_eq!(c("languages")["partial"].as_array().unwrap().len(), 1);
    assert_eq!(c("languages")["partial"][0]["proficiency"], "basic");
    assert_eq!(c("experience")["required_years"], 4);
    assert_eq!(c("experience")["years"], 2.0);
    assert_eq!(c("academic_level")["points"], 15.0);
    m.db.cleanup().await;
}

/// MA4/MA5 en la API: ninguna ruta del listado deja ordenar ni filtrar por edad ni género, y el
/// match no cambia con el nombre (lo único «personal» que hay en la cuenta).
#[tokio::test]
async fn ma4_el_nombre_de_la_persona_no_cambia_el_puntaje() {
    let m = mundo().await;
    let t = tres_ofertas(&m).await;
    let ana = m.persona("ana").await;
    tiene_skill(&m, &ana, &skill(&m, 0).await, "advanced").await;
    let antes = detalle(&m, &ana, &t.o1).await["match"].clone();
    m.db.exec(&format!(
        "UPDATE users SET first_name = 'Zoe', last_name = 'Zeta' WHERE id = '{}'",
        ana.id
    ))
    .await
    .unwrap();
    assert_eq!(detalle(&m, &ana, &t.o1).await["match"], antes);
    for q in [
        "sort=age",
        "sort=gender",
        "age=30",
        "gender=f",
        "marital_status=x",
    ] {
        let r = lista(&m, &ana, q).await;
        assert_eq!(r.json["error"], "parametro_no_permitido", "{q}");
    }
    m.db.cleanup().await;
}

// ================================================================== lado reclutador

struct Rec {
    m: Mundo,
    ana: Persona,
    rec: Persona,
    oferta: String,
}

/// Una oferta que pide: habilidad 0 obligatoria (avanzada), habilidad 1 opcional, el idioma 0
/// (intermedio) y un nivel académico mínimo (el 1). Ana tiene la habilidad 0 avanzada, el idioma
/// en nativo y un nivel por encima.
///   Todo a la vista:  skills 2/3·50 = 33.3 + idioma 15 + nivel 15 = 63.3 de 80 → 79.
///   Solo habilidades: 33.3 de 50 → 67.
///   Habilidades y nivel: 48.3 de 65 → 74.
async fn escenario() -> Rec {
    let m = mundo().await;
    let emp = empresa(&m, "cafenuevo").await;
    let o = oferta(&m, &emp, "Analista", 0).await;
    pide_skill(&m, &o, &skill(&m, 0).await, true, Some("advanced")).await;
    pide_skill(&m, &o, &skill(&m, 1).await, false, None).await;
    pide_idioma(&m, &o, &idioma(&m, 0).await, "intermediate").await;
    m.db.exec(&format!(
        "UPDATE job_offers SET minimum_academic_level_id = '{}' WHERE id = '{o}'",
        nivel(&m, 1).await
    ))
    .await
    .unwrap();
    let ana = m.persona("ana").await;
    tiene_skill(&m, &ana, &skill(&m, 0).await, "advanced").await;
    tiene_idioma(&m, &ana, &idioma(&m, 0).await, "native").await;
    tiene_nivel(&m, &ana, &nivel(&m, 3).await).await;
    tiene_experiencia(&m, &ana, "2020-01-01").await;
    let rec = m.persona("rec").await;
    m.dar_rol(&rec.id, "recruiter").await;
    m.db.exec(&format!(
        "INSERT INTO company_recruiters (id, user_id, company_id, role) \
         VALUES (gen_random_uuid(), '{}', '{emp}', 'recruiter')",
        rec.id
    ))
    .await
    .unwrap();
    Rec {
        m,
        ana,
        rec,
        oferta: o,
    }
}

impl Rec {
    async fn aplicar(&self, campos: &[&str]) -> String {
        let r = self
            .m
            .enviar(
                "POST",
                &format!("/api/v1/offers/{}/applications", self.oferta),
                Some(&self.ana.token),
                Some(json!({"consent": true, "shared_fields": campos})),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "aplicar: {}", r.text);
        r.json["id"].as_str().unwrap().to_string()
    }

    async fn panel(&self, app: &str) -> Value {
        let r = self
            .m
            .get(&format!("/api/v1/company/applications/{app}"), &self.rec)
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
        r.json
    }
}

const MINIMO: [&str; 3] = ["display_name", "skills", "experience"];

fn con(extra: &[&'static str]) -> Vec<&'static str> {
    let mut v = MINIMO.to_vec();
    v.extend_from_slice(extra);
    v
}

/// MA9: sin `match_score` marcado, nada de matching cruza al reclutador.
#[tokio::test]
async fn ma9_sin_compartirlo_ningun_dato_de_matching_llega_al_reclutador() {
    let e = escenario().await;
    let app = e.aplicar(&MINIMO).await;
    // Ana sí ve su puntaje.
    assert_eq!(detalle(&e.m, &e.ana, &e.oferta).await["match"]["score"], 79);
    let d = e.panel(&app).await;
    assert!(d["applicant"].get("match_score").is_none(), "{d}");
    let lista =
        e.m.get(
            &format!("/api/v1/company/offers/{}/applications", e.oferta),
            &e.rec,
        )
        .await;
    for fila in lista.json["items"].as_array().unwrap() {
        assert!(
            fila.get("match_score").is_none() && fila["applicant"].get("match_score").is_none()
        );
    }
    for ruta in [
        "/api/v1/company/offers",
        &format!("/api/v1/company/offers/{}", e.oferta),
    ] {
        let r = e.m.get(ruta, &e.rec).await;
        assert!(!r.text.contains("match"), "{ruta}: {}", r.text);
    }
    e.m.db.cleanup().await;
}

/// `match_score` es un nombre más del conjunto cerrado del consentimiento.
#[tokio::test]
async fn match_score_es_un_campo_compartible_y_se_guarda() {
    let e = escenario().await;
    let app = e.aplicar(&con(&["match_score"])).await;
    let r =
        e.m.get(&format!("/api/v1/me/applications/{app}"), &e.ana)
            .await;
    assert_eq!(
        r.json["shared_fields"],
        json!(["display_name", "skills", "experience", "match_score"])
    );
    // Sigue sin existir `interview_score`, y sin los obligatorios no hay aplicación.
    let otra = e.m.persona("otra").await;
    for malo in [con(&["interview_score"]), vec!["match_score"]] {
        let r =
            e.m.enviar(
                "POST",
                &format!("/api/v1/offers/{}/applications", e.oferta),
                Some(&otra.token),
                Some(json!({"consent": true, "shared_fields": malo})),
            )
            .await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{malo:?}");
    }
    e.m.db.cleanup().await;
}

/// MA9: compartido, el reclutador ve el puntaje calculado **solo con lo que se compartió**.
#[tokio::test]
async fn ma9_el_reclutador_ve_un_puntaje_hecho_solo_con_lo_consentido() {
    for (campos, esperado) in [
        (con(&["match_score"]), 67),
        (con(&["match_score", "education"]), 74),
        (con(&["match_score", "languages", "education"]), 79),
    ] {
        let e = escenario().await;
        let app = e.aplicar(&campos).await;
        let d = e.panel(&app).await;
        let ms = &d["applicant"]["match_score"];
        assert_eq!(ms["score"], esperado, "{campos:?}: {d}");
        // Sin lo no compartido: el componente no aplica y lo dice, no lo cuenta como faltante.
        let idiomas = componente(ms, "languages");
        if campos.contains(&"languages") {
            assert_eq!(idiomas["applies"], true);
            assert_eq!(idiomas["matched"].as_array().unwrap().len(), 1);
        } else {
            assert_eq!(idiomas["applies"], false);
            assert_eq!(idiomas["reason"], "no_compartido");
            assert!(idiomas.get("missing").is_none());
        }
        let academico = componente(ms, "academic_level");
        assert_eq!(academico["applies"], campos.contains(&"education"));
        e.m.db.cleanup().await;
    }
}

/// El puntaje del reclutador sigue lo que la persona tiene hoy (se ve en vivo, como el resto).
#[tokio::test]
async fn el_puntaje_del_reclutador_sigue_el_perfil_vivo() {
    let e = escenario().await;
    let app = e.aplicar(&con(&["match_score"])).await;
    assert_eq!(e.panel(&app).await["applicant"]["match_score"]["score"], 67);
    tiene_skill(&e.m, &e.ana, &skill(&e.m, 1).await, "beginner").await;
    assert_eq!(
        e.panel(&app).await["applicant"]["match_score"]["score"],
        100
    );
    e.m.db.cleanup().await;
}

/// Retirar la aplicación quita también el puntaje (el panel ya no muestra a la persona).
#[tokio::test]
async fn retirar_quita_el_puntaje_del_panel() {
    let e = escenario().await;
    let app = e.aplicar(&con(&["match_score"])).await;
    let r =
        e.m.enviar(
            "POST",
            &format!("/api/v1/me/applications/{app}/withdraw"),
            Some(&e.ana.token),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let d = e.panel(&app).await;
    assert!(d["applicant"].is_null());
    assert!(!d.to_string().contains("match_score"), "{d}");
    e.m.db.cleanup().await;
}

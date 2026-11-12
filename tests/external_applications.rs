//! Registro privado de aplicaciones externas (HU-32, decisión 6): CRUD de la persona, filtrado
//! por dueño, cifrado de las notas, tope de 200. PostgreSQL real, una base por prueba. Escritas
//! antes que el código.

mod common;
#[path = "common/empresas.rs"]
mod empresas;

use axum::http::StatusCode;
use empresas::{mundo, pii, Persona, Resp};
use serde_json::{json, Value};

const RUTA: &str = "/api/v1/me/external-applications";

fn cuerpo() -> Value {
    json!({"company_name": "Banco X", "position": "Analista", "applied_on": "2026-11-05",
           "status": "interviewing", "notes": "NOTA-PRIVADA-EXTERNA"})
}

fn codigo(r: &Resp) -> &str {
    r.json["error"].as_str().unwrap_or("")
}

async fn crear(m: &empresas::Mundo, p: &Persona, c: Value) -> Resp {
    m.enviar("POST", RUTA, Some(&p.token), Some(c)).await
}

#[tokio::test]
async fn crear_leer_actualizar_y_borrar() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let r = crear(&m, &ana, cuerpo()).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    let id = r.json["id"].as_str().unwrap().to_string();
    assert!(r.json["created_at"].is_string());
    assert_eq!(r.json["company_name"], "Banco X");
    assert_eq!(r.json["notes"], "NOTA-PRIVADA-EXTERNA");

    let g = m.get(&format!("{RUTA}/{id}"), &ana).await;
    assert_eq!(g.status, StatusCode::OK);
    assert_eq!(g.json, r.json);

    let mut nuevo = cuerpo();
    nuevo["status"] = json!("offer");
    nuevo.as_object_mut().unwrap().remove("notes");
    for _ in 0..2 {
        let p = m
            .enviar(
                "PUT",
                &format!("{RUTA}/{id}"),
                Some(&ana.token),
                Some(nuevo.clone()),
            )
            .await;
        assert_eq!(p.status, StatusCode::NO_CONTENT, "{}", p.text);
    }
    let g = m.get(&format!("{RUTA}/{id}"), &ana).await;
    assert_eq!(g.json["status"], "offer");
    assert!(g.json["notes"].is_null());

    let d = m
        .enviar("DELETE", &format!("{RUTA}/{id}"), Some(&ana.token), None)
        .await;
    assert_eq!(d.status, StatusCode::NO_CONTENT);
    assert_eq!(
        m.get(&format!("{RUTA}/{id}"), &ana).await.status,
        StatusCode::NOT_FOUND
    );
    m.db.cleanup().await;
}

/// Privado: lo de otra persona es 404 para leer, cambiar y borrar; la lista no lo mezcla.
#[tokio::test]
async fn lo_ajeno_es_404_y_la_lista_es_solo_mia() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let beto = m.persona("beto").await;
    let id = crear(&m, &ana, cuerpo()).await.json["id"]
        .as_str()
        .unwrap()
        .to_string();
    for ruta in [
        format!("{RUTA}/{id}"),
        format!("{RUTA}/00000000-0000-7000-8000-000000000000"),
        format!("{RUTA}/nope"),
    ] {
        assert_eq!(
            m.get(&ruta, &beto).await.status,
            StatusCode::NOT_FOUND,
            "{ruta}"
        );
        let p = m
            .enviar("PUT", &ruta, Some(&beto.token), Some(cuerpo()))
            .await;
        assert_eq!(p.status, StatusCode::NOT_FOUND);
        let d = m.enviar("DELETE", &ruta, Some(&beto.token), None).await;
        assert_eq!(d.status, StatusCode::NOT_FOUND);
    }
    assert_eq!(m.n("SELECT count(*) FROM external_applications").await, 1);
    assert_eq!(
        m.get(RUTA, &beto).await.json["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    // Y sin token, 401.
    assert_eq!(
        m.enviar("GET", RUTA, None, None).await.status,
        StatusCode::UNAUTHORIZED
    );
    m.db.cleanup().await;
}

/// Orden por `applied_on` descendente, filtro por estado, cursor y parámetros cerrados.
#[tokio::test]
async fn lista_ordenada_filtrada_y_paginada() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    for (i, (f, s)) in [
        ("2026-11-01", "applied"),
        ("2026-11-03", "rejected"),
        ("2026-11-02", "applied"),
        ("2026-11-03", "applied"),
    ]
    .iter()
    .enumerate()
    {
        let mut c = cuerpo();
        c["applied_on"] = json!(f);
        c["status"] = json!(s);
        c["position"] = json!(format!("Puesto {i}"));
        assert_eq!(crear(&m, &ana, c).await.status, StatusCode::CREATED);
    }
    let mut fechas = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let q = cursor
            .as_ref()
            .map_or("?limit=3".to_string(), |c| format!("?limit=3&cursor={c}"));
        let r = m.get(&format!("{RUTA}{q}"), &ana).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
        fechas.extend(
            r.json["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["applied_on"].as_str().unwrap().to_string()),
        );
        match r.json["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    assert_eq!(
        fechas,
        ["2026-11-03", "2026-11-03", "2026-11-02", "2026-11-01"]
    );
    let r = m.get(&format!("{RUTA}?status=rejected"), &ana).await;
    assert_eq!(r.json["items"].as_array().unwrap().len(), 1);
    let r = m.get(&format!("{RUTA}?status=hired"), &ana).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = m.get(&format!("{RUTA}?q=banco"), &ana).await;
    assert_eq!(codigo(&r), "parametro_no_permitido");
    m.db.cleanup().await;
}

#[tokio::test]
async fn validacion_de_la_entrada() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let mut c = cuerpo();
    c["status"] = json!("hired");
    assert_eq!(codigo(&crear(&m, &ana, c).await), "datos_invalidos");
    let mut c = cuerpo();
    c["applied_on"] = json!("2999-01-01");
    assert_eq!(codigo(&crear(&m, &ana, c).await), "datos_invalidos");
    let mut c = cuerpo();
    c["applicant_id"] = json!("x");
    assert_eq!(codigo(&crear(&m, &ana, c).await), "campo_no_permitido");
    assert_eq!(m.n("SELECT count(*) FROM external_applications").await, 0);
    m.db.cleanup().await;
}

/// `notes` va cifrada en reposo.
#[tokio::test]
async fn las_notas_se_guardan_cifradas() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    crear(&m, &ana, cuerpo()).await;
    let bytes = common::bytes_de(&m.db, "SELECT notes FROM external_applications").await;
    assert!(!common::contiene(&bytes, b"NOTA-PRIVADA"));
    assert_eq!(pii().decrypt(&bytes).unwrap(), "NOTA-PRIVADA-EXTERNA");
    m.db.cleanup().await;
}

/// Máximo 200 por persona (409), y otra persona no se ve afectada.
#[tokio::test]
async fn maximo_200_por_persona() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let beto = m.persona("beto").await;
    m.db.exec(&format!(
        "INSERT INTO external_applications (id, applicant_id, company_name, position, applied_on, status) \
         SELECT gen_random_uuid(), (SELECT id FROM applicants WHERE user_id = '{}'), 'Empresa', 'Puesto', current_date, 'applied' \
         FROM generate_series(1, 200)",
        ana.id
    ))
    .await
    .unwrap();
    let r = crear(&m, &ana, cuerpo()).await;
    assert_eq!(
        (r.status, codigo(&r)),
        (StatusCode::CONFLICT, "limite_de_registros")
    );
    assert_eq!(m.n("SELECT count(*) FROM external_applications").await, 200);
    assert_eq!(crear(&m, &beto, cuerpo()).await.status, StatusCode::CREATED);
    m.db.cleanup().await;
}

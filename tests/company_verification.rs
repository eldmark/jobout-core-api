//! Fase 5a, paso 3: verificación de la empresa y membresía por la plataforma (`company.verify`),
//! la auditoría de cada acción y el control de «verificada y vigente» que usarán las rutas de
//! publicar (EM2, EM6, EM7). Sin reloj: «hoy» se inyecta como fecha o sale de la base.

mod common;
#[path = "common/empresas.rs"]
mod empresas;

use axum::http::StatusCode;
use empresas::{capturar_log, mundo, Mundo, Persona};
use jobout_core_api::company_gate::{self, Gate};
use serde_json::{json, Value};

const NIT: &str = "1234567-8";
const ID_NO: &str = "00000000-0000-7000-8000-000000000000";

async fn escenario() -> (Mundo, Persona, Persona, String) {
    let m = mundo().await;
    let (ana, id) = m.admin_con_empresa("ana", NIT).await;
    let admin = m.admin("plataforma").await;
    (m, ana, admin, id)
}

async fn hoy(m: &Mundo, dias: i32) -> String {
    m.texto(&format!(
        "SELECT ((now() AT TIME ZONE 'UTC')::date + {dias})::text"
    ))
    .await
}

async fn membresia(m: &Mundo, admin: &Persona, id: &str, ini: &str, fin: &str) -> empresas::Resp {
    m.enviar(
        "PUT",
        &format!("/api/v1/admin/companies/{id}/membership"),
        Some(&admin.token),
        Some(json!({"membership_type": "business", "start_date": ini, "expiration_date": fin})),
    )
    .await
}

async fn auditoria(m: &Mundo, accion: &str) -> i64 {
    m.n(&format!(
        "SELECT count(*) FROM access_audit WHERE action = '{accion}'"
    ))
    .await
}

async fn estado(m: &Mundo, p: &Persona) -> Value {
    m.get("/api/v1/company", p).await.json["membership"]["status"].clone()
}

// ------------------------------------------------------------------ permisos

#[tokio::test]
async fn solo_el_admin_de_plataforma_entra_y_no_lee_nada_mas() {
    let (m, ana, admin, id) = escenario().await;
    let cand = m.persona("cand").await;
    for p in [&ana, &cand] {
        for (metodo, ruta) in [
            ("GET", "/api/v1/admin/companies".to_string()),
            ("POST", format!("/api/v1/admin/companies/{id}/verify")),
            ("POST", format!("/api/v1/admin/companies/{id}/unverify")),
            ("PUT", format!("/api/v1/admin/companies/{id}/membership")),
        ] {
            let r = m
                .enviar(metodo, &ruta, Some(&p.token), Some(json!({})))
                .await;
            assert_eq!(
                (r.status, r.json["error"].as_str()),
                (StatusCode::FORBIDDEN, Some("sin_permiso")),
                "{metodo} {ruta}"
            );
        }
    }
    assert_eq!(
        m.n("SELECT count(*) FROM companies WHERE verified_at IS NOT NULL")
            .await,
        0
    );
    assert_eq!(
        m.n("SELECT count(*) FROM access_audit").await,
        0,
        "lo rechazado no deja auditoría de acción"
    );
    // El admin tiene `company.verify` y nada más; y no es dueño de ninguna empresa.
    let me = m.get("/api/v1/me", &admin).await.json;
    assert_eq!(me["permissions"], json!(["company.verify"]));
    assert_eq!(
        m.get("/api/v1/company", &admin).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        m.get("/api/v1/company/offers", &admin).await.status,
        StatusCode::NOT_FOUND
    );
    m.db.cleanup().await;
}

// ------------------------------------------------------------------ listar

#[tokio::test]
async fn la_lista_trae_lo_necesario_para_revisar_y_nunca_el_correo_completo() {
    let (m, ana, admin, id) = escenario().await;
    m.miembro(&ana, "rec", "recruiter").await;
    let (_beto, id_b) = m.admin_con_empresa("beto", "7654321-0").await;
    m.db.exec(&format!("UPDATE companies SET verified_at = now(), created_at = created_at + interval '1 hour' WHERE id = '{id_b}'")).await.unwrap();
    let r = m
        .enviar_crudo(
            "GET",
            "/api/v1/admin/companies",
            Some(&admin.token),
            vec![],
            &[("x-forwarded-for", "203.0.113.77")],
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let items = r.json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(r.json["next_cursor"], Value::Null);
    // Más antiguas primero.
    assert_eq!(items[0]["id"], id);
    assert_eq!(items[1]["id"], id_b);
    let a = &items[0];
    let mut k: Vec<&String> = a.as_object().unwrap().keys().collect();
    k.sort();
    assert_eq!(
        k,
        [
            "created_at",
            "creator_email_domain",
            "id",
            "name",
            "recruiters_count",
            "tax_id",
            "verified_at",
            "website"
        ]
    );
    assert_eq!(a["tax_id"], NIT);
    assert_eq!(a["creator_email_domain"], "correo-prueba.com");
    assert_eq!(a["recruiters_count"], 2);
    assert_eq!(a["verified_at"], Value::Null);
    assert!(items[1]["verified_at"].is_string());
    assert!(
        !r.text.contains("ana@"),
        "correo completo en la lista: {}",
        r.text
    );
    // Una fila de auditoría, sin contenido y con la IP hasheada.
    assert_eq!(auditoria(&m, "company.list").await, 1);
    assert_eq!(m.n("SELECT count(*) FROM access_audit WHERE action = 'company.list' AND resource_type = 'company' AND resource_id IS NULL AND ip_hash IS NOT NULL").await, 1);
    let fila = m.texto("SELECT row(a.*)::text FROM access_audit a").await;
    for prohibido in [NIT, "Empresa ana", "203.0.113.77", "correo-prueba"] {
        assert!(!fila.contains(prohibido), "{prohibido}: {fila}");
    }
    let hash = common::bytes_de(&m.db, "SELECT ip_hash FROM access_audit").await;
    assert!(!common::contiene(&hash, b"203.0.113.77"));
    assert_eq!(
        m.n(&format!(
            "SELECT count(*) FROM access_audit WHERE actor_user_id = '{}'",
            admin.id
        ))
        .await,
        1
    );
    m.db.cleanup().await;
}

#[tokio::test]
async fn la_lista_filtra_por_verificada_pagina_y_cierra_los_parametros() {
    let (m, _ana, admin, _id) = escenario().await;
    let (_b, id_b) = m.admin_con_empresa("beto", "7654321-0").await;
    m.admin_con_empresa("carla", "1111111-1").await;
    m.db.exec(&format!(
        "UPDATE companies SET verified_at = now() WHERE id = '{id_b}'"
    ))
    .await
    .unwrap();
    let n = |r: &empresas::Resp| r.json["items"].as_array().unwrap().len();
    assert_eq!(
        n(&m.get("/api/v1/admin/companies?verified=true", &admin).await),
        1
    );
    assert_eq!(
        n(&m.get("/api/v1/admin/companies?verified=false", &admin)
            .await),
        2
    );
    assert_eq!(n(&m.get("/api/v1/admin/companies", &admin).await), 3);
    let p = m.get("/api/v1/admin/companies?limit=2", &admin).await.json;
    assert_eq!(p["items"].as_array().unwrap().len(), 2);
    let c = p["next_cursor"].as_str().unwrap();
    let p2 = m
        .get(
            &format!("/api/v1/admin/companies?limit=2&cursor={c}"),
            &admin,
        )
        .await
        .json;
    assert_eq!(p2["items"].as_array().unwrap().len(), 1);
    assert_eq!(p2["next_cursor"], Value::Null);
    for malo in [
        "verified=quizas",
        "verified=",
        "limit=0",
        "limit=51",
        "cursor=zzz",
        "verified=true&verified=false",
    ] {
        let r = m
            .get(&format!("/api/v1/admin/companies?{malo}"), &admin)
            .await;
        assert_eq!(
            (r.status, r.json["error"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some("datos_invalidos")),
            "{malo}"
        );
    }
    let antes = auditoria(&m, "company.list").await;
    for prohibido in [
        "age=30",
        "gender=f",
        "name=ana",
        "sort=name",
        "q=cafe",
        "status=x",
        "tax_id=1",
    ] {
        let r = m
            .get(&format!("/api/v1/admin/companies?{prohibido}"), &admin)
            .await;
        assert_eq!(
            (r.status, r.json["error"].as_str()),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Some("parametro_no_permitido")
            ),
            "{prohibido}"
        );
        assert_eq!(
            r.json["message"],
            "La consulta trae un parámetro que no se admite."
        );
    }
    assert_eq!(
        auditoria(&m, "company.list").await,
        antes,
        "un parámetro rechazado no llega a la base"
    );
    m.db.cleanup().await;
}

// ------------------------------------------------------------- verificar

#[tokio::test]
async fn verificar_es_idempotente_audita_una_vez_y_no_mueve_la_fecha() {
    let (m, ana, admin, id) = escenario().await;
    let ruta = format!("/api/v1/admin/companies/{id}/verify");
    let r = m.enviar("POST", &ruta, Some(&admin.token), None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    assert!(r.text.is_empty());
    let c = m.get("/api/v1/company", &ana).await.json;
    assert_eq!(c["verified"], true);
    assert!(c["verified_at"].is_string());
    let fecha = m
        .texto(&format!(
            "SELECT verified_at::text FROM companies WHERE id = '{id}'"
        ))
        .await;
    assert_eq!(auditoria(&m, "company.verify").await, 1);
    assert_eq!(m.n(&format!("SELECT count(*) FROM access_audit WHERE action = 'company.verify' AND resource_type = 'company' AND resource_id = '{id}' AND actor_user_id = '{}'", admin.id)).await, 1);
    // Repetir: 204, sin segunda fila y sin mover `verified_at`.
    let r = m.enviar("POST", &ruta, Some(&admin.token), None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(auditoria(&m, "company.verify").await, 1);
    assert_eq!(
        m.texto(&format!(
            "SELECT verified_at::text FROM companies WHERE id = '{id}'"
        ))
        .await,
        fecha
    );
    m.db.cleanup().await;
}

#[tokio::test]
async fn quitar_la_verificacion_limpia_la_fecha_audita_y_es_idempotente() {
    let (m, ana, admin, id) = escenario().await;
    m.enviar(
        "POST",
        &format!("/api/v1/admin/companies/{id}/verify"),
        Some(&admin.token),
        None,
    )
    .await;
    let ruta = format!("/api/v1/admin/companies/{id}/unverify");
    for _ in 0..2 {
        let r = m.enviar("POST", &ruta, Some(&admin.token), None).await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    }
    let c = m.get("/api/v1/company", &ana).await.json;
    assert_eq!(
        (c["verified"].clone(), c["verified_at"].clone()),
        (json!(false), Value::Null)
    );
    assert_eq!(auditoria(&m, "company.unverify").await, 1);
    // Los borradores y la empresa se conservan.
    assert_eq!(m.n("SELECT count(*) FROM companies").await, 1);
    m.db.cleanup().await;
}

#[tokio::test]
async fn verificar_lo_inexistente_o_mal_formado_es_el_mismo_404() {
    let (m, _ana, admin, _id) = escenario().await;
    let mut vistos = vec![];
    for accion in ["verify", "unverify"] {
        for id in [ID_NO, "no-es-uuid", "123"] {
            let r = m
                .enviar(
                    "POST",
                    &format!("/api/v1/admin/companies/{id}/{accion}"),
                    Some(&admin.token),
                    None,
                )
                .await;
            vistos.push((r.status, r.json));
        }
    }
    let r = membresia(&m, &admin, ID_NO, "2026-01-01", "2026-12-31").await;
    vistos.push((r.status, r.json));
    assert_eq!(vistos[0].0, StatusCode::NOT_FOUND);
    assert_eq!(vistos[0].1["error"], "no_encontrado");
    assert!(vistos.iter().all(|v| v == &vistos[0]), "{vistos:?}");
    assert_eq!(
        m.n("SELECT count(*) FROM access_audit").await,
        0,
        "sin recurso no hay acción que auditar"
    );
    m.db.cleanup().await;
}

#[tokio::test]
async fn un_admin_de_plataforma_no_verifica_la_empresa_de_la_que_es_miembro() {
    let (m, _ana, admin, id) = escenario().await;
    m.db.exec(&format!(
        "INSERT INTO company_recruiters (id, user_id, company_id, role) VALUES (gen_random_uuid(), '{}', '{id}', 'recruiter')",
        admin.id
    )).await.unwrap();
    let r = m
        .enviar(
            "POST",
            &format!("/api/v1/admin/companies/{id}/verify"),
            Some(&admin.token),
            None,
        )
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("sin_permiso"))
    );
    assert_eq!(
        m.n("SELECT count(*) FROM companies WHERE verified_at IS NOT NULL")
            .await,
        0
    );
    assert_eq!(auditoria(&m, "company.verify").await, 0);
    m.db.cleanup().await;
}

/// Sin rastro no hay acción: si la auditoría falla, la operación falla (500) y no cambia nada.
#[tokio::test]
async fn si_la_auditoria_falla_la_accion_falla_y_no_queda_cambiada() {
    let (m, _ana, admin, id) = escenario().await;
    m.db.exec("ALTER TABLE access_audit RENAME TO access_audit_rota")
        .await
        .unwrap();
    let r = m
        .enviar(
            "POST",
            &format!("/api/v1/admin/companies/{id}/verify"),
            Some(&admin.token),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        m.n("SELECT count(*) FROM companies WHERE verified_at IS NOT NULL")
            .await,
        0
    );
    let r = membresia(&m, &admin, &id, "2026-01-01", "2026-12-31").await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(m.n("SELECT count(*) FROM memberships").await, 0);
    let r = m.get("/api/v1/admin/companies", &admin).await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
    m.db.exec("ALTER TABLE access_audit_rota RENAME TO access_audit")
        .await
        .unwrap();
    m.db.exec(&format!(
        "UPDATE companies SET verified_at = now() WHERE id = '{id}'"
    ))
    .await
    .unwrap();
    let r = m
        .enviar(
            "POST",
            &format!("/api/v1/admin/companies/{id}/unverify"),
            Some(&admin.token),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    m.db.exec("ALTER TABLE access_audit RENAME TO access_audit_rota")
        .await
        .unwrap();
    m.db.exec(&format!(
        "UPDATE companies SET verified_at = now() WHERE id = '{id}'"
    ))
    .await
    .unwrap();
    let r = m
        .enviar(
            "POST",
            &format!("/api/v1/admin/companies/{id}/unverify"),
            Some(&admin.token),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        m.n("SELECT count(*) FROM companies WHERE verified_at IS NOT NULL")
            .await,
        1,
        "la verificación sigue"
    );
    m.db.cleanup().await;
}

// ------------------------------------------------------------ membresía

#[tokio::test]
async fn la_membresia_se_crea_se_audita_y_reemplaza_a_la_vigente() {
    let (m, ana, admin, id) = escenario().await;
    let (ini, fin) = (hoy(&m, -10).await, hoy(&m, 355).await);
    let r = membresia(&m, &admin, &id, &ini, &fin).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    let c = m.get("/api/v1/company", &ana).await.json;
    assert_eq!(
        c["membership"],
        json!({"type": "business", "start_date": ini, "expiration_date": fin, "status": "active"})
    );
    assert_eq!(auditoria(&m, "company.membership_set").await, 1);
    // Reemplazar: la anterior queda cancelada y solo hay una vigente.
    let (ini2, fin2) = (hoy(&m, 0).await, hoy(&m, 30).await);
    assert_eq!(
        membresia(&m, &admin, &id, &ini2, &fin2).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        m.n("SELECT count(*) FROM memberships WHERE status = 'active'")
            .await,
        1
    );
    assert_eq!(
        m.n("SELECT count(*) FROM memberships WHERE status = 'cancelled'")
            .await,
        1
    );
    assert_eq!(
        m.get("/api/v1/company", &ana).await.json["membership"]["expiration_date"],
        fin2
    );
    assert_eq!(auditoria(&m, "company.membership_set").await, 2);
    // Verificar no crea membresía.
    let (_b, id_b) = m.admin_con_empresa("beto", "7654321-0").await;
    m.enviar(
        "POST",
        &format!("/api/v1/admin/companies/{id_b}/verify"),
        Some(&admin.token),
        None,
    )
    .await;
    assert_eq!(
        m.n(&format!(
            "SELECT count(*) FROM memberships WHERE company_id = '{id_b}'"
        ))
        .await,
        0
    );
    m.db.cleanup().await;
}

#[tokio::test]
async fn el_estado_de_la_membresia_se_calcula_por_fechas() {
    let (m, ana, admin, id) = escenario().await;
    for (ini, fin, esperado) in [
        (-20, -1, "expired"),
        (1, 20, "expired"),
        (-1, 0, "active"),
        (0, 0, "active"),
    ] {
        let (i, f) = (hoy(&m, ini).await, hoy(&m, fin).await);
        assert_eq!(
            membresia(&m, &admin, &id, &i, &f).await.status,
            StatusCode::NO_CONTENT
        );
        assert_eq!(estado(&m, &ana).await, esperado, "{ini}..{fin}");
        // La columna sigue `active`: el estado sale de las fechas, no de ella.
        assert_eq!(
            m.n("SELECT count(*) FROM memberships WHERE status = 'active'")
                .await,
            1
        );
    }
    m.db.cleanup().await;
}

#[tokio::test]
async fn la_membresia_valida_el_cuerpo() {
    let (m, _ana, admin, id) = escenario().await;
    let u = StatusCode::UNPROCESSABLE_ENTITY;
    let ruta = format!("/api/v1/admin/companies/{id}/membership");
    let mut casos: Vec<(Value, &str)> = vec![];
    let ok = |extra: Value| {
        let mut c = json!({"membership_type": "business", "start_date": "2026-01-01", "expiration_date": "2026-12-31"});
        for (k, v) in extra.as_object().unwrap() {
            c[k] = v.clone();
        }
        c
    };
    casos.push((
        ok(json!({"expiration_date": "2025-12-31"})),
        "datos_invalidos",
    ));
    // 400 días exactos valen; 401, no. (2026-01-01 + 400 = 2027-02-05).
    casos.push((
        ok(json!({"expiration_date": "2027-02-06"})),
        "datos_invalidos",
    ));
    casos.push((
        ok(json!({"membership_type": "enterprise"})),
        "datos_invalidos",
    ));
    casos.push((ok(json!({"membership_type": "free"})), "datos_invalidos"));
    casos.push((ok(json!({"membership_type": 5})), "datos_invalidos"));
    casos.push((ok(json!({"start_date": "2026-02-30"})), "datos_invalidos"));
    casos.push((ok(json!({"start_date": "ayer"})), "datos_invalidos"));
    casos.push((ok(json!({"start_date": null})), "datos_invalidos"));
    casos.push((ok(json!({"status": "active"})), "campo_no_permitido"));
    casos.push((ok(json!({"company_id": id})), "campo_no_permitido"));
    for (c, codigo) in casos {
        let r = m
            .enviar("PUT", &ruta, Some(&admin.token), Some(c.clone()))
            .await;
        assert_eq!(
            (r.status, r.json["error"].as_str()),
            (u, Some(codigo)),
            "{c}: {}",
            r.text
        );
    }
    let r = m
        .enviar(
            "PUT",
            &ruta,
            Some(&admin.token),
            Some(ok(json!({"expiration_date": "2027-02-05"}))),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    let r = m
        .enviar(
            "PUT",
            &ruta,
            Some(&admin.token),
            Some(ok(json!({"expiration_date": "2026-01-01"}))),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "inicio = fin vale");
    assert_eq!(m.n("SELECT count(*) FROM memberships").await, 2);
    assert_eq!(
        auditoria(&m, "company.membership_set").await,
        2,
        "las rechazadas no auditan acción"
    );
    m.db.cleanup().await;
}

// ----------------------------------------------- «verificada y vigente»

/// EM2, EM6, EM7: el control que usarán publicar, reanudar y el panel. Gana
/// `empresa_no_verificada` sobre `membresia_vencida`. «Hoy» se inyecta.
#[tokio::test]
async fn el_control_de_publicacion_distingue_no_verificada_y_membresia_vencida() {
    let (m, _ana, _admin, id) = escenario().await;
    let uid: uuid::Uuid = id.parse().unwrap();
    let hoy_ = "2026-06-15";
    let check = |dia: &'static str| {
        let db = m.db.conn.clone();
        async move { company_gate::check_on(&db, uid, dia).await.unwrap() }
    };
    // Sin verificar y sin membresía: gana la verificación.
    assert_eq!(check(hoy_).await, Err(Gate::NotVerified));
    m.db.exec(&format!(
        "UPDATE companies SET verified_at = now() WHERE id = '{id}'"
    ))
    .await
    .unwrap();
    // Verificada sin membresía: vencida.
    assert_eq!(check(hoy_).await, Err(Gate::MembershipExpired));
    let alta = |ini: &str, fin: &str, st: &str| {
        format!("INSERT INTO memberships (id, company_id, membership_type_id, start_date, expiration_date, status) \
                 VALUES (gen_random_uuid(), '{id}', (SELECT id FROM membership_types WHERE code = 'business'), '{ini}', '{fin}', '{st}')")
    };
    m.db.exec(&alta("2026-06-16", "2026-12-31", "active"))
        .await
        .unwrap();
    assert_eq!(
        check(hoy_).await,
        Err(Gate::MembershipExpired),
        "aún no empieza"
    );
    assert_eq!(
        check("2026-06-16").await,
        Ok(()),
        "el día de inicio ya vale"
    );
    assert_eq!(
        check("2026-12-31").await,
        Ok(()),
        "el día de vencimiento todavía vale"
    );
    assert_eq!(check("2027-01-01").await, Err(Gate::MembershipExpired));
    // Cancelada: no vale aunque las fechas cubran hoy.
    m.db.exec("UPDATE memberships SET status = 'cancelled'")
        .await
        .unwrap();
    assert_eq!(check("2026-07-01").await, Err(Gate::MembershipExpired));
    m.db.exec("UPDATE memberships SET status = 'active'")
        .await
        .unwrap();
    assert_eq!(check("2026-07-01").await, Ok(()));
    // Quitar la verificación corta todo, con membresía vigente.
    m.db.exec(&format!(
        "UPDATE companies SET verified_at = NULL WHERE id = '{id}'"
    ))
    .await
    .unwrap();
    assert_eq!(check("2026-07-01").await, Err(Gate::NotVerified));
    // Una empresa que no existe tampoco pasa.
    assert_eq!(
        company_gate::check_on(&m.db.conn, uuid::Uuid::now_v7(), hoy_)
            .await
            .unwrap(),
        Err(Gate::NotVerified)
    );
    m.db.cleanup().await;
}

/// El mismo control con el reloj real: verificada y con membresía de hoy, pasa.
#[tokio::test]
async fn el_control_con_el_reloj_de_la_base_pasa_con_membresia_de_hoy() {
    let (m, _ana, admin, id) = escenario().await;
    let uid: uuid::Uuid = id.parse().unwrap();
    assert_eq!(
        company_gate::check(&m.db.conn, uid).await.unwrap(),
        Err(Gate::NotVerified)
    );
    m.enviar(
        "POST",
        &format!("/api/v1/admin/companies/{id}/verify"),
        Some(&admin.token),
        None,
    )
    .await;
    assert_eq!(
        company_gate::check(&m.db.conn, uid).await.unwrap(),
        Err(Gate::MembershipExpired)
    );
    let (i, f) = (hoy(&m, -1).await, hoy(&m, 30).await);
    membresia(&m, &admin, &id, &i, &f).await;
    assert_eq!(company_gate::check(&m.db.conn, uid).await.unwrap(), Ok(()));
    let (i, f) = (hoy(&m, -30).await, hoy(&m, -1).await);
    membresia(&m, &admin, &id, &i, &f).await;
    assert_eq!(
        company_gate::check(&m.db.conn, uid).await.unwrap(),
        Err(Gate::MembershipExpired)
    );
    m.db.cleanup().await;
}

// ------------------------------------------------------------------- logs

#[tokio::test]
async fn el_log_y_la_auditoria_no_llevan_el_nit_ni_el_nombre_de_la_empresa() {
    let (log, _guarda) = capturar_log();
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let c = m
        .crear_empresa(&ana, "MARCA-NIT-5521", "MARCA Razon Social")
        .await;
    let id = c["id"].as_str().unwrap().to_string();
    let admin = m.admin("plataforma").await;
    let mut fuentes: Vec<String> = vec![];
    let l = m.get("/api/v1/admin/companies", &admin).await;
    assert!(
        l.text.contains("MARCA-NIT-5521"),
        "el admin sí ve el NIT en la lista"
    );
    for (metodo, ruta) in [
        ("POST", format!("/api/v1/admin/companies/{id}/verify")),
        ("POST", format!("/api/v1/admin/companies/{id}/unverify")),
        ("POST", format!("/api/v1/admin/companies/{ID_NO}/verify")),
    ] {
        fuentes.push(m.enviar(metodo, &ruta, Some(&admin.token), None).await.text);
    }
    fuentes.push(
        membresia(&m, &admin, &id, "2026-01-01", "2026-12-31")
            .await
            .text,
    );
    // Un fallo interno en la lista (tabla rota) tampoco filtra.
    m.db.exec("ALTER TABLE access_audit RENAME TO access_audit_rota")
        .await
        .unwrap();
    fuentes.push(m.get("/api/v1/admin/companies", &admin).await.text);
    m.db.exec("ALTER TABLE access_audit_rota RENAME TO access_audit")
        .await
        .unwrap();
    fuentes.push(
        m.texto("SELECT coalesce(string_agg(row(a.*)::text, ' '), '') FROM access_audit a")
            .await,
    );
    let texto = log.texto();
    assert!(texto.contains("captura-activa"));
    fuentes.push(texto);
    for f in fuentes {
        for secreto in [
            "MARCA-NIT",
            "5521",
            "MARCA Razon",
            "MARCA",
            ana.token.as_str(),
            admin.token.as_str(),
        ] {
            assert!(!f.contains(secreto), "se filtró {secreto:?}: {f}");
        }
    }
    m.db.cleanup().await;
}

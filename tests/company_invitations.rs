//! Fase 5a, paso 3: invitaciones de reclutadores (EM4, EM5, RO6, SX3, SX6). Token de un solo
//! uso, guardado solo como SHA-256, atado al correo del invitado. Sin reloj: los plazos se
//! mueven en las filas.

mod common;
#[path = "common/empresas.rs"]
mod empresas;

use axum::http::StatusCode;
use empresas::{capturar_log, mundo, Mundo, Persona};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const NIT: &str = "1234567-8";

async fn con_admin() -> (Mundo, Persona, String) {
    let m = mundo().await;
    let (ana, id) = m.admin_con_empresa("ana", NIT).await;
    (m, ana, id)
}

fn sha(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn invitacion(m: &Mundo, admin: &Persona, email: &str, rol: &str) -> (String, String) {
    let i = m.invitar(admin, email, rol).await;
    (
        i["id"].as_str().unwrap().to_string(),
        i["token"].as_str().unwrap().to_string(),
    )
}

// ------------------------------------------------------------- crear

#[tokio::test]
async fn em5_la_invitacion_devuelve_el_token_una_vez_y_en_la_base_solo_queda_su_hash() {
    let (m, ana, id) = con_admin().await;
    let r = m
        .enviar(
            "POST",
            "/api/v1/company/invitations",
            Some(&ana.token),
            Some(json!({"email": "Nuevo@Correo-Prueba.com", "role": "recruiter"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    let token = r.json["token"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 43);
    assert!(token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    assert_eq!(
        (r.json["role"].as_str(), r.json["status"].as_str()),
        (Some("recruiter"), Some("pending"))
    );
    assert!(empresas::uuid_valido(r.json["id"].as_str().unwrap()));
    assert!(r.json["expires_at"].as_str().unwrap().ends_with('Z'));
    assert!(r.json["created_at"].as_str().unwrap().ends_with('Z'));
    let mut k: Vec<&String> = r.json.as_object().unwrap().keys().collect();
    k.sort();
    assert_eq!(
        k,
        ["created_at", "expires_at", "id", "role", "status", "token"]
    );
    assert!(
        !r.text.to_lowercase().contains("correo-prueba"),
        "la respuesta repite el correo"
    );

    // Base: solo el SHA-256 del token y el HMAC del correo (normalizado); el token no está en
    // ninguna columna; vigencia exacta de 7 días contada desde la creación.
    let hash = common::bytes_de(&m.db, "SELECT token_hash FROM company_invitations").await;
    assert_eq!(common::hex(&hash), sha(&token));
    let correo = common::bytes_de(&m.db, "SELECT email_hash FROM company_invitations").await;
    assert_eq!(
        correo,
        common::llaves().email_hash("nuevo@correo-prueba.com")
    );
    let fila = m
        .texto("SELECT row(i.*)::text FROM company_invitations i")
        .await;
    assert!(
        !fila.contains(&token) && !fila.to_lowercase().contains("correo-prueba"),
        "{fila}"
    );
    assert_eq!(m.n("SELECT count(*) FROM company_invitations WHERE expires_at - created_at = interval '7 days' AND used_at IS NULL AND revoked_at IS NULL").await, 1);
    assert_eq!(m.n(&format!("SELECT count(*) FROM company_invitations WHERE company_id = '{id}' AND created_by = '{}'", ana.id)).await, 1);
    // Dos invitaciones al mismo correo son dos tokens distintos.
    let (_, otro) = invitacion(&m, &ana, "nuevo@correo-prueba.com", "recruiter").await;
    assert_ne!(token, otro);
    m.db.cleanup().await;
}

#[tokio::test]
async fn la_invitacion_valida_el_cuerpo_y_cierra_los_roles() {
    let (m, ana, _) = con_admin().await;
    let u = StatusCode::UNPROCESSABLE_ENTITY;
    let largo = format!("{}@correo.com", "a".repeat(250));
    for (cuerpo, codigo) in [
        (json!({"email": "x@correo.com"}), "datos_invalidos"),
        (json!({"role": "recruiter"}), "datos_invalidos"),
        (
            json!({"email": "x@correo.com", "role": "admin"}),
            "datos_invalidos",
        ),
        (
            json!({"email": "x@correo.com", "role": "applicant"}),
            "datos_invalidos",
        ),
        (
            json!({"email": "x@correo.com", "role": "COMPANY_ADMIN"}),
            "datos_invalidos",
        ),
        (
            json!({"email": "x@correo.com", "role": 5}),
            "datos_invalidos",
        ),
        (
            json!({"email": "no-es-correo", "role": "recruiter"}),
            "datos_invalidos",
        ),
        (
            json!({"email": "a b@correo.com", "role": "recruiter"}),
            "datos_invalidos",
        ),
        (
            json!({"email": largo, "role": "recruiter"}),
            "datos_invalidos",
        ),
        (json!({"email": 5, "role": "recruiter"}), "datos_invalidos"),
        (
            json!({"email": "x@correo.com", "role": "recruiter", "company_id": "x"}),
            "campo_no_permitido",
        ),
        (
            json!({"email": "x@correo.com", "role": "recruiter", "token": "x"}),
            "campo_no_permitido",
        ),
        (
            json!({"email": "x@correo.com", "role": "recruiter", "expires_at": "x"}),
            "campo_no_permitido",
        ),
    ] {
        let r = m
            .enviar(
                "POST",
                "/api/v1/company/invitations",
                Some(&ana.token),
                Some(cuerpo.clone()),
            )
            .await;
        assert_eq!(
            (r.status, r.json["error"].as_str()),
            (u, Some(codigo)),
            "{cuerpo}: {}",
            r.text
        );
        assert!(!r.text.contains("x@correo.com"));
    }
    let r = m
        .enviar_crudo(
            "POST",
            "/api/v1/company/invitations",
            Some(&ana.token),
            b"{no".to_vec(),
            &[],
        )
        .await;
    assert_eq!(r.status, u);
    assert_eq!(m.n("SELECT count(*) FROM company_invitations").await, 0);
    // Los dos roles válidos.
    for rol in ["recruiter", "company_admin"] {
        let i = m.invitar(&ana, &format!("{rol}@correo.com"), rol).await;
        assert_eq!(i["role"], rol);
    }
    m.db.cleanup().await;
}

#[tokio::test]
async fn invitar_a_alguien_de_otra_empresa_responde_igual_que_a_alguien_nuevo() {
    let (m, ana, _) = con_admin().await;
    let (beto, _) = m.admin_con_empresa("beto", "7654321-0").await;
    let a = m
        .enviar(
            "POST",
            "/api/v1/company/invitations",
            Some(&ana.token),
            Some(json!({"email": beto.email, "role": "recruiter"})),
        )
        .await;
    let b = m
        .enviar(
            "POST",
            "/api/v1/company/invitations",
            Some(&ana.token),
            Some(json!({"email": "nadie@correo-prueba.com", "role": "recruiter"})),
        )
        .await;
    assert_eq!(
        (a.status, b.status),
        (StatusCode::CREATED, StatusCode::CREATED)
    );
    let forma = |v: &Value| {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        (k, v["status"].clone(), v["role"].clone())
    };
    assert_eq!(forma(&a.json), forma(&b.json));
    // Quien ya está en MI empresa sí recibe 409: es la misma empresa de quien invita.
    let rec = m.miembro(&ana, "rec", "recruiter").await;
    let r = m
        .enviar(
            "POST",
            "/api/v1/company/invitations",
            Some(&ana.token),
            Some(json!({"email": rec.email, "role": "recruiter"})),
        )
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::CONFLICT, Some("ya_tiene_empresa"))
    );
    m.db.cleanup().await;
}

#[tokio::test]
async fn invitar_sin_empresa_pero_con_el_permiso_es_404() {
    let m = mundo().await;
    let carla = m.persona("carla").await;
    m.dar_rol(&carla.id, "company_admin").await;
    let r = m
        .enviar(
            "POST",
            "/api/v1/company/invitations",
            Some(&carla.token),
            Some(json!({"email": "x@correo.com", "role": "recruiter"})),
        )
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("no_encontrado"))
    );
    m.db.cleanup().await;
}

// --------------------------------------------------------------- listar

#[tokio::test]
async fn el_listado_no_lleva_token_ni_correo_y_calcula_el_estado() {
    let (m, ana, _) = con_admin().await;
    let (beto, _) = m.admin_con_empresa("beto", "7654321-0").await;
    let (id_pend, token) = invitacion(&m, &ana, "uno@correo.com", "recruiter").await;
    let (id_usada, _) = invitacion(&m, &ana, "dos@correo.com", "recruiter").await;
    let (id_venc, _) = invitacion(&m, &ana, "tres@correo.com", "company_admin").await;
    let (id_rev, _) = invitacion(&m, &ana, "cuatro@correo.com", "recruiter").await;
    invitacion(&m, &beto, "ajena@correo.com", "recruiter").await;
    m.db.exec(&format!(
        "UPDATE company_invitations SET used_at = now() WHERE id = '{id_usada}'"
    ))
    .await
    .unwrap();
    m.db.exec(&format!("UPDATE company_invitations SET expires_at = now() - interval '1 second' WHERE id = '{id_venc}'")).await.unwrap();
    let r = m
        .enviar(
            "DELETE",
            &format!("/api/v1/company/invitations/{id_rev}"),
            Some(&ana.token),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);

    let r = m.get("/api/v1/company/invitations", &ana).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        !r.text.contains(&token) && !r.text.contains("\"token\"") && !r.text.contains("correo.com")
    );
    let items = r.json["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        3,
        "la revocada y las de otra empresa no salen: {}",
        r.text
    );
    let estado = |id: &str| items.iter().find(|i| i["id"] == id).unwrap()["status"].clone();
    assert_eq!(estado(&id_pend), "pending");
    assert_eq!(estado(&id_usada), "used");
    assert_eq!(estado(&id_venc), "expired");
    for i in items {
        let mut k: Vec<&String> = i.as_object().unwrap().keys().collect();
        k.sort();
        assert_eq!(k, ["created_at", "expires_at", "id", "role", "status"]);
    }
    // Paginación y consulta cerrada.
    let p = m
        .get("/api/v1/company/invitations?limit=2", &ana)
        .await
        .json;
    assert_eq!(p["items"].as_array().unwrap().len(), 2);
    let c = p["next_cursor"].as_str().unwrap();
    let p2 = m
        .get(
            &format!("/api/v1/company/invitations?limit=2&cursor={c}"),
            &ana,
        )
        .await
        .json;
    assert_eq!(p2["items"].as_array().unwrap().len(), 1);
    assert_eq!(p2["next_cursor"], Value::Null);
    let r = m.get("/api/v1/company/invitations?status=used", &ana).await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("parametro_no_permitido")
        )
    );
    let r = m.get("/api/v1/company/invitations?limit=0", &ana).await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("datos_invalidos"))
    );
    m.db.cleanup().await;
}

// -------------------------------------------------------------- aceptar

#[tokio::test]
async fn aceptar_da_el_rol_de_empresa_y_gasta_la_invitacion() {
    let (m, ana, id) = con_admin().await;
    let luis = m.persona("luis").await;
    let (inv, token) = invitacion(&m, &ana, "LUIS@correo-prueba.com", "recruiter").await;
    let r = m.aceptar(&luis, &token).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json["company"]["id"], id);
    assert_eq!(r.json["company"]["name"], "Empresa ana");
    assert_eq!(r.json["role"], "recruiter");
    assert_eq!(r.json.as_object().unwrap().len(), 2);
    assert_eq!(r.json["company"].as_object().unwrap().len(), 2);
    let me = m.get("/api/v1/me", &luis).await.json;
    assert_eq!(me["roles"], json!(["applicant", "recruiter"]));
    assert!(!me["permissions"]
        .as_array()
        .unwrap()
        .contains(&json!("company.invite")));
    assert_eq!(
        m.get("/api/v1/company", &luis).await.json["my_role"],
        "recruiter"
    );
    assert_eq!(
        m.n(&format!(
            "SELECT count(*) FROM company_invitations WHERE id = '{inv}' AND used_at IS NOT NULL"
        ))
        .await,
        1
    );
    // Un solo uso, también para quien la gastó.
    let r = m.aceptar(&luis, &token).await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("invitacion_invalida"))
    );
    assert_eq!(
        m.n("SELECT count(*) FROM company_recruiters WHERE deleted_at IS NULL")
            .await,
        2
    );
    m.db.cleanup().await;
}

#[tokio::test]
async fn aceptar_como_company_admin_da_los_permisos_de_administrador() {
    let (m, ana, _) = con_admin().await;
    let luis = m.persona("luis").await;
    let (_, token) = invitacion(&m, &ana, &luis.email, "company_admin").await;
    assert_eq!(m.aceptar(&luis, &token).await.json["role"], "company_admin");
    let me = m.get("/api/v1/me", &luis).await.json;
    assert_eq!(me["roles"], json!(["applicant", "company_admin"]));
    assert!(me["permissions"]
        .as_array()
        .unwrap()
        .contains(&json!("company.invite")));
    assert_eq!(
        m.get("/api/v1/company/recruiters", &luis).await.status,
        StatusCode::OK
    );
    m.db.cleanup().await;
}

/// Las cinco causas de fallo dan exactamente la misma respuesta, y ninguna revela la empresa.
#[tokio::test]
async fn una_invitacion_invalida_es_siempre_el_mismo_404() {
    let (m, ana, _) = con_admin().await;
    let luis = m.persona("luis").await;
    let intruso = m.persona("intruso").await;
    let (_, ok) = invitacion(&m, &ana, &luis.email, "recruiter").await;
    let (id_v, vencida) = invitacion(&m, &ana, &luis.email, "recruiter").await;
    let (_, usada) = invitacion(&m, &ana, &luis.email, "recruiter").await;
    let (id_r, revocada) = invitacion(&m, &ana, &luis.email, "recruiter").await;
    m.db.exec(&format!("UPDATE company_invitations SET expires_at = now() - interval '1 second' WHERE id = '{id_v}'")).await.unwrap();
    m.db.exec(&format!(
        "UPDATE company_invitations SET used_at = now() WHERE token_hash = '\\x{}'::bytea",
        sha(&usada)
    ))
    .await
    .unwrap();
    m.enviar(
        "DELETE",
        &format!("/api/v1/company/invitations/{id_r}"),
        Some(&ana.token),
        None,
    )
    .await;
    let desconocido = "x".repeat(43);
    let mut vistos: Vec<(String, StatusCode, Value)> = vec![];
    for (quien, token, caso) in [
        (&luis, &desconocido, "desconocido"),
        (&luis, &vencida, "vencida"),
        (&luis, &usada, "usada"),
        (&luis, &revocada, "revocada"),
        (&intruso, &ok, "de otro correo"),
    ] {
        let r = m.aceptar(quien, token).await;
        vistos.push((caso.to_string(), r.status, r.json));
    }
    assert_eq!(vistos[0].1, StatusCode::NOT_FOUND);
    assert_eq!(vistos[0].2["error"], "invitacion_invalida");
    for v in &vistos {
        assert_eq!((v.1, &v.2), (vistos[0].1, &vistos[0].2), "{}", v.0);
        assert!(!v.2.to_string().contains("Empresa"), "{}", v.0);
    }
    // Un intento ajeno no gasta la invitación: su dueño todavía puede usarla.
    assert_eq!(m.n("SELECT count(*) FROM company_invitations WHERE used_at IS NULL AND revoked_at IS NULL AND expires_at > now()").await, 1);
    assert_eq!(m.aceptar(&luis, &ok).await.status, StatusCode::OK);
    assert_eq!(
        m.n("SELECT count(*) FROM company_recruiters WHERE deleted_at IS NULL")
            .await,
        2
    );
    m.db.cleanup().await;
}

#[tokio::test]
async fn aceptar_con_empresa_propia_es_409_y_no_gasta_la_invitacion() {
    let (m, ana, _) = con_admin().await;
    let (beto, _) = m.admin_con_empresa("beto", "7654321-0").await;
    let (inv, token) = invitacion(&m, &ana, &beto.email, "recruiter").await;
    let r = m.aceptar(&beto, &token).await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::CONFLICT, Some("ya_tiene_empresa"))
    );
    assert_eq!(
        m.n(&format!(
            "SELECT count(*) FROM company_invitations WHERE id = '{inv}' AND used_at IS NULL"
        ))
        .await,
        1
    );
    assert_eq!(
        m.get("/api/v1/company", &beto).await.json["name"],
        "Empresa beto"
    );
    m.db.cleanup().await;
}

#[tokio::test]
async fn aceptar_valida_el_cuerpo_antes_de_mirar_el_token() {
    let (m, ana, _) = con_admin().await;
    for (cuerpo, codigo) in [
        (json!({}), "datos_invalidos"),
        (json!({"token": 5}), "datos_invalidos"),
        (json!({"token": ""}), "datos_invalidos"),
        (json!({"token": "a".repeat(201)}), "datos_invalidos"),
        (
            json!({"token": "abc", "company_id": "x"}),
            "campo_no_permitido",
        ),
        (
            json!({"token": "abc", "role": "company_admin"}),
            "campo_no_permitido",
        ),
    ] {
        let r = m
            .enviar(
                "POST",
                "/api/v1/invitations/accept",
                Some(&ana.token),
                Some(cuerpo.clone()),
            )
            .await;
        assert_eq!(
            (r.status, r.json["error"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some(codigo)),
            "{cuerpo}"
        );
    }
    // Los cuerpos inválidos no consumen intentos: no hay filas de límite.
    assert_eq!(
        m.n("SELECT count(*) FROM rate_events WHERE kind = 'invitation_accept'")
            .await,
        0
    );
    m.db.cleanup().await;
}

#[tokio::test]
async fn aceptar_esta_limitado_a_10_intentos_por_persona_y_hora_acierte_o_no() {
    let (m, ana, _) = con_admin().await;
    let luis = m.persona("luis").await;
    let otro = m.persona("otro").await;
    let (_, bueno) = invitacion(&m, &ana, &luis.email, "recruiter").await;
    for i in 0..10 {
        let r = m.aceptar(&luis, &format!("token-malo-{i}")).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "intento {i}");
    }
    // El undécimo, aunque el token sea el bueno: 429 con Retry-After, y no se consume.
    let r = m.aceptar(&luis, &bueno).await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("demasiados_intentos"))
    );
    let espera: u64 = r.headers["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=3600).contains(&espera), "{espera}");
    assert_eq!(
        m.n("SELECT count(*) FROM company_invitations WHERE used_at IS NULL")
            .await,
        1
    );
    // Es por persona.
    assert_eq!(
        m.aceptar(&otro, "otro-token").await.status,
        StatusCode::NOT_FOUND
    );
    // Pasada la hora (se mueve el registro) vuelve a poder.
    m.db.exec("UPDATE rate_events SET created_at = created_at - interval '2 hours'")
        .await
        .unwrap();
    assert_eq!(m.aceptar(&luis, &bueno).await.status, StatusCode::OK);
    m.db.cleanup().await;
}

// ------------------------------------------------------------- revocar

#[tokio::test]
async fn revocar_deja_inservible_la_invitacion() {
    let (m, ana, _) = con_admin().await;
    let luis = m.persona("luis").await;
    let (id, token) = invitacion(&m, &ana, &luis.email, "recruiter").await;
    let r = m
        .enviar(
            "DELETE",
            &format!("/api/v1/company/invitations/{id}"),
            Some(&ana.token),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        m.n(&format!(
            "SELECT count(*) FROM company_invitations WHERE id = '{id}' AND revoked_at IS NOT NULL"
        ))
        .await,
        1
    );
    assert_eq!(m.aceptar(&luis, &token).await.status, StatusCode::NOT_FOUND);
    // Revocarla otra vez, o una ya usada: 404.
    let r = m
        .enviar(
            "DELETE",
            &format!("/api/v1/company/invitations/{id}"),
            Some(&ana.token),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    m.db.cleanup().await;
}

// ---------------------------------------------------------------- límites

#[tokio::test]
async fn se_pueden_crear_20_invitaciones_por_empresa_y_dia_y_el_21_es_429() {
    let (m, ana, _) = con_admin().await;
    let (beto, _) = m.admin_con_empresa("beto", "7654321-0").await;
    for i in 0..20 {
        let (id, _) = invitacion(&m, &ana, &format!("p{i}@correo.com"), "recruiter").await;
        // Revocar no devuelve el cupo del día.
        if i % 2 == 0 {
            m.enviar(
                "DELETE",
                &format!("/api/v1/company/invitations/{id}"),
                Some(&ana.token),
                None,
            )
            .await;
        }
    }
    let r = m
        .enviar(
            "POST",
            "/api/v1/company/invitations",
            Some(&ana.token),
            Some(json!({"email": "p21@correo.com", "role": "recruiter"})),
        )
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("demasiados_intentos"))
    );
    let espera: u64 = r.headers["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=86_400).contains(&espera));
    assert_eq!(m.n("SELECT count(*) FROM company_invitations").await, 20);
    // Es por empresa.
    m.invitar(&beto, "x@correo.com", "recruiter").await;
    // Pasado el día vuelve a poder.
    m.db.exec("UPDATE company_invitations SET created_at = created_at - interval '2 days'")
        .await
        .unwrap();
    m.invitar(&ana, "p21@correo.com", "recruiter").await;
    m.db.cleanup().await;
}

#[tokio::test]
async fn no_puede_haber_mas_de_50_pendientes() {
    let (m, ana, id) = con_admin().await;
    // 50 pendientes de hace días (no cuentan para el tope diario), más una usada y una vencida.
    m.db.exec(&format!(
        "INSERT INTO company_invitations (id, company_id, email_hash, role, token_hash, expires_at, created_at) \
         SELECT gen_random_uuid(), '{id}', sha256(i::text::bytea), 'recruiter', sha256(('t' || i)::bytea), \
                now() + interval '3 days', now() - interval '3 days' FROM generate_series(1, 50) i"
    )).await.unwrap();
    m.db.exec(&format!(
        "INSERT INTO company_invitations (id, company_id, email_hash, role, token_hash, expires_at, created_at, used_at) \
         VALUES (gen_random_uuid(), '{id}', '\\x01', 'recruiter', '\\x02', now() + interval '3 days', now() - interval '3 days', now())"
    )).await.unwrap();
    let r = m
        .enviar(
            "POST",
            "/api/v1/company/invitations",
            Some(&ana.token),
            Some(json!({"email": "x@correo.com", "role": "recruiter"})),
        )
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("datos_invalidos")),
        "{}",
        r.text
    );
    // Una vencida deja de ser pendiente y libera un lugar.
    m.db.exec("UPDATE company_invitations SET expires_at = now() - interval '1 second' WHERE id = (SELECT id FROM company_invitations WHERE used_at IS NULL LIMIT 1)").await.unwrap();
    m.invitar(&ana, "x@correo.com", "recruiter").await;
    m.db.cleanup().await;
}

// ---------------------------------------------------------- concurrencia

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dos_aceptaciones_simultaneas_de_la_misma_invitacion_dejan_una() {
    let (m, ana, _) = con_admin().await;
    for ronda in 0..3 {
        let luis = m.persona(&format!("luis{ronda}")).await;
        let (_, token) = invitacion(&m, &ana, &luis.email, "recruiter").await;
        let (a, b) = tokio::join!(m.aceptar(&luis, &token), m.aceptar(&luis, &token));
        let mut estados = [a.status, b.status];
        estados.sort();
        assert_eq!(
            estados,
            [StatusCode::OK, StatusCode::NOT_FOUND],
            "ronda {ronda}: {} / {}",
            a.text,
            b.text
        );
        let malo = if a.status == StatusCode::NOT_FOUND {
            &a
        } else {
            &b
        };
        assert_eq!(malo.json["error"], "invitacion_invalida");
        assert_eq!(m.n(&format!("SELECT count(*) FROM company_recruiters WHERE user_id = '{}' AND deleted_at IS NULL", luis.id)).await, 1);
    }
    m.db.cleanup().await;
}

// ------------------------------------------------------ roles y escalada

/// RO6: no hay forma de ganar `admin` ni `company_admin` por la API fuera de crear la empresa o
/// una invitación que un administrador emitió.
#[tokio::test]
async fn ro6_nadie_se_escala_por_la_api() {
    let (m, ana, _) = con_admin().await;
    let rec = m.miembro(&ana, "rec", "recruiter").await;
    // Un reclutador no puede invitar (ni a sí mismo como administrador).
    let r = m
        .enviar(
            "POST",
            "/api/v1/company/invitations",
            Some(&rec.token),
            Some(json!({"email": rec.email, "role": "company_admin"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Una invitación de otra empresa, de otro correo, no sirve para entrar.
    let (beto, _) = m.admin_con_empresa("beto", "7654321-0").await;
    let (_, token) = invitacion(&m, &beto, "otra-persona@correo.com", "company_admin").await;
    assert_eq!(m.aceptar(&rec, &token).await.status, StatusCode::NOT_FOUND);
    // `admin` no aparece nunca en las cuentas de empresa.
    assert_eq!(m.n("SELECT count(*) FROM user_roles ur JOIN roles r ON r.id = ur.role_id WHERE r.code = 'admin'").await, 0);
    let me = m.get("/api/v1/me", &rec).await.json;
    assert_eq!(me["roles"], json!(["applicant", "recruiter"]));
    m.db.cleanup().await;
}

// ------------------------------------------------------------------- logs

#[tokio::test]
async fn el_log_y_los_errores_no_llevan_el_token_ni_el_correo_invitado() {
    let (log, _guarda) = capturar_log();
    let m = mundo().await;
    let (ana, _) = m.admin_con_empresa("ana", NIT).await;
    let luis = m
        .persona_con_email("marca.invitado@correo-prueba.com")
        .await;
    let mut fuentes: Vec<String> = vec![];
    let i = m.invitar(&ana, &luis.email, "recruiter").await;
    let token = i["token"].as_str().unwrap().to_string();
    fuentes.push(i["id"].to_string());
    let r = m
        .enviar(
            "POST",
            "/api/v1/company/invitations",
            Some(&ana.token),
            Some(json!({"email": "marca.invitado@correo-prueba.com", "role": "otro"})),
        )
        .await;
    fuentes.push(r.text);
    let malo = "MARCA-TOKEN-NO-VALE-0000000000000000000000";
    fuentes.push(m.aceptar(&luis, malo).await.text);
    let intruso = m.persona("intruso").await;
    fuentes.push(m.aceptar(&intruso, &token).await.text);
    fuentes.push(m.get("/api/v1/company/invitations", &ana).await.text);
    assert_eq!(m.aceptar(&luis, &token).await.status, StatusCode::OK);
    // Fallo interno en medio de una aceptación.
    let (_, t2) = invitacion(&m, &ana, "marca.invitado2@correo-prueba.com", "recruiter").await;
    let otro = m
        .persona_con_email("marca.invitado2@correo-prueba.com")
        .await;
    m.db.exec("ALTER TABLE company_recruiters RENAME TO company_recruiters_rota")
        .await
        .unwrap();
    let r = m.aceptar(&otro, &t2).await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
    fuentes.push(r.text);

    let texto = log.texto();
    assert!(texto.contains("captura-activa"), "no se capturó ningún log");
    fuentes.push(texto);
    for f in fuentes {
        for secreto in [
            token.as_str(),
            t2.as_str(),
            malo,
            "marca.invitado",
            "MARCA-TOKEN",
        ] {
            assert!(!f.contains(secreto), "se filtró {secreto:?}: {f}");
        }
    }
    m.db.cleanup().await;
}

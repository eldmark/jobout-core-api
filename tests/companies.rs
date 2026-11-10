//! Fase 5a, paso 3: empresas y reclutadores (EM1, EM3, EM4, EM8, EM9, RO6, SX2, SX3, SX6).
//! Contra PostgreSQL real; una base por prueba. Ningún reloj: los plazos se mueven en la base.

mod common;
#[path = "common/empresas.rs"]
mod empresas;

use axum::http::StatusCode;
use empresas::{capturar_log, mundo, Mundo};
use serde_json::{json, Value};

const NIT: &str = "1234567-8";

fn llaves() -> jobout_core_api::crypto::PiiKeys {
    common::llaves()
}

// ------------------------------------------------------------------ EM1

#[tokio::test]
async fn em1_crear_la_empresa_la_deja_sin_verificar_y_hace_admin_a_quien_la_crea() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let c = m.crear_empresa(&ana, NIT, "Café Nuevo S.A.").await;
    assert!(empresas::uuid_valido(c["id"].as_str().unwrap()));
    assert_eq!(c["name"], "Café Nuevo S.A.");
    assert_eq!(c["tax_id"], NIT);
    assert_eq!(c["website"], "https://cafenuevo.example");
    assert_eq!(c["verified"], false);
    assert_eq!(c["verified_at"], Value::Null);
    assert_eq!(c["membership"], Value::Null);
    assert_eq!(c["my_role"], "company_admin");
    assert!(c["created_at"].as_str().unwrap().ends_with('Z'));
    let mut claves: Vec<&String> = c.as_object().unwrap().keys().collect();
    claves.sort();
    assert_eq!(
        claves,
        [
            "country_id",
            "created_at",
            "description",
            "employment_sector_id",
            "id",
            "membership",
            "my_role",
            "name",
            "phone",
            "tax_id",
            "verified",
            "verified_at",
            "website"
        ]
    );
    // Base: sin verificar, con su fila de reclutador administrador y el rol.
    assert_eq!(
        m.n("SELECT count(*) FROM companies WHERE verified_at IS NULL")
            .await,
        1
    );
    assert_eq!(
        m.n("SELECT count(*) FROM company_recruiters WHERE role = 'company_admin' AND deleted_at IS NULL")
            .await,
        1
    );
    // /me: conserva applicant y suma company_admin con sus permisos.
    let me = m.get("/api/v1/me", &ana).await.json;
    assert_eq!(me["roles"], json!(["applicant", "company_admin"]));
    let permisos: Vec<&str> = me["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    for p in ["company.update", "company.invite", "offer.publish"] {
        assert!(permisos.contains(&p), "falta {p}");
    }
    assert!(!permisos.contains(&"company.verify"));
    m.db.cleanup().await;
}

/// Mutación «guardar el NIT en claro»: la fila no contiene el NIT, ni normalizado, y el hash
/// es el HMAC del NIT normalizado con la clave de hash.
#[tokio::test]
async fn el_nit_se_guarda_cifrado_y_con_hmac() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    m.crear_empresa(&ana, "ab-12 cd", "Café").await;
    let fila = common::bytes_de(&m.db, "SELECT tax_id FROM companies").await;
    for claro in ["ab-12 cd", "AB12CD", "ab12cd", "12"] {
        assert!(
            !common::contiene(&fila, claro.as_bytes()),
            "NIT en claro: {claro}"
        );
    }
    let hash = common::bytes_de(&m.db, "SELECT tax_id_hash FROM companies").await;
    assert_eq!(hash.len(), 32);
    assert!(!common::contiene(&hash, b"AB12CD"));
    assert_eq!(llaves().decrypt(&fila).unwrap(), "ab-12 cd");
    // Todo el registro, en texto, sin el NIT.
    let todo = m.texto("SELECT row(c.*)::text FROM companies c").await;
    assert!(
        !todo.contains("ab-12") && !todo.contains("AB12CD"),
        "{todo}"
    );
    m.db.cleanup().await;
}

// ------------------------------------------------------------------ EM3

#[tokio::test]
async fn em3_un_nit_repetido_es_409_aunque_cambie_el_formato() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let beto = m.persona("beto").await;
    m.crear_empresa(&ana, NIT, "Café Nuevo").await;
    for otro in [NIT, "12345678", "1234567 8", " 1234567-8 "] {
        let r = m
            .enviar(
                "POST",
                "/api/v1/companies",
                Some(&beto.token),
                Some(m.cuerpo_empresa(otro, "Otra").await),
            )
            .await;
        assert_eq!(r.status, StatusCode::CONFLICT, "{otro}: {}", r.text);
        assert_eq!(r.json["error"], "empresa_duplicada");
        assert!(
            !r.text.contains("Café Nuevo") && !r.text.contains("1234567"),
            "{}",
            r.text
        );
    }
    // Las letras se normalizan a mayúsculas.
    m.crear_empresa(&beto, "ab-12 cd", "Beto").await;
    let carla = m.persona("carla").await;
    let r = m
        .enviar(
            "POST",
            "/api/v1/companies",
            Some(&carla.token),
            Some(m.cuerpo_empresa("AB12CD", "Carla").await),
        )
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::CONFLICT, Some("empresa_duplicada"))
    );
    assert_eq!(m.n("SELECT count(*) FROM companies").await, 2);
    m.db.cleanup().await;
}

#[tokio::test]
async fn em3_la_misma_persona_no_crea_dos_empresas_y_gana_ya_tiene_empresa() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    m.crear_empresa(&ana, NIT, "Una").await;
    // Mismo NIT o distinto: la persona ya tiene empresa, y eso se dice primero.
    for nit in [NIT, "9999999-9"] {
        let r = m
            .enviar(
                "POST",
                "/api/v1/companies",
                Some(&ana.token),
                Some(m.cuerpo_empresa(nit, "Dos").await),
            )
            .await;
        assert_eq!(r.status, StatusCode::CONFLICT);
        assert_eq!(r.json["error"], "ya_tiene_empresa");
    }
    assert_eq!(m.n("SELECT count(*) FROM companies").await, 1);
    // Un cuerpo inválido sigue siendo 422 antes que cualquier 409.
    let r = m
        .enviar(
            "POST",
            "/api/v1/companies",
            Some(&ana.token),
            Some(json!({"name": "x"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    m.db.cleanup().await;
}

// ------------------------------------------------------- validación

async fn probar(m: &Mundo, token: &str, cambio: Value, estado: StatusCode, codigo: &str) {
    let mut cuerpo = m.cuerpo_empresa(NIT, "Café Nuevo").await;
    for (k, v) in cambio.as_object().unwrap() {
        if v == &json!("__quitar__") {
            cuerpo.as_object_mut().unwrap().remove(k);
        } else {
            cuerpo[k] = v.clone();
        }
    }
    let r = m
        .enviar(
            "POST",
            "/api/v1/companies",
            Some(token),
            Some(cuerpo.clone()),
        )
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (estado, Some(codigo)),
        "{cambio}: {}",
        r.text
    );
    // Texto fijo: jamás repite lo enviado.
    for (_, v) in cambio.as_object().unwrap() {
        if let Some(s) = v.as_str() {
            if s.len() >= 6 && s != "__quitar__" {
                assert!(
                    !r.text.contains(s),
                    "el error repite lo enviado: {}",
                    r.text
                );
            }
        }
    }
    assert_eq!(m.n("SELECT count(*) FROM companies").await, 0, "{cambio}");
}

#[tokio::test]
async fn la_creacion_valida_el_cuerpo_y_no_guarda_nada() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let t = ana.token.as_str();
    let u = StatusCode::UNPROCESSABLE_ENTITY;
    let largo = |n: usize| "a".repeat(n);
    for cambio in [
        json!({"name": "a"}),
        json!({"name": largo(121)}),
        json!({"name": "   "}),
        json!({"name": "__quitar__"}),
        json!({"name": 5}),
        json!({"tax_id": "123"}),
        json!({"tax_id": "1".repeat(21)}),
        json!({"tax_id": "12345$67"}),
        json!({"tax_id": "- - -"}),
        json!({"tax_id": "__quitar__"}),
        json!({"website": "http://cafenuevo.example"}),
        json!({"website": format!("https://{}", largo(200))}),
        json!({"website": "https://con espacio.example"}),
        json!({"phone": "1".repeat(21)}),
        json!({"description": largo(2001)}),
        json!({"country_id": "no-es-uuid"}),
        json!({"country_id": "00000000-0000-7000-8000-000000000000"}),
        json!({"country_id": "__quitar__"}),
        json!({"employment_sector_id": "__quitar__"}),
        json!({"employment_sector_id": 7}),
    ] {
        probar(&m, t, cambio, u, "datos_invalidos").await;
    }
    // Catálogos inactivos: el id existe, pero no se puede elegir (como en el perfil).
    let cuerpo = m.cuerpo_empresa(NIT, "Café").await;
    for tabla in ["countries", "employment_sectors"] {
        m.db.exec(&format!("UPDATE {tabla} SET active = false"))
            .await
            .unwrap();
        let r = m
            .enviar("POST", "/api/v1/companies", Some(t), Some(cuerpo.clone()))
            .await;
        assert_eq!(
            (r.status, r.json["error"].as_str()),
            (u, Some("datos_invalidos")),
            "{tabla}"
        );
        m.db.exec(&format!("UPDATE {tabla} SET active = true"))
            .await
            .unwrap();
    }
    assert_eq!(m.n("SELECT count(*) FROM companies").await, 0);
    m.db.cleanup().await;
}

#[tokio::test]
async fn los_campos_opcionales_pueden_omitirse_o_ir_null() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let mut cuerpo = m.cuerpo_empresa(NIT, "  Café Nuevo  ").await;
    let o = cuerpo.as_object_mut().unwrap();
    o.remove("website");
    o.insert("phone".into(), Value::Null);
    o.remove("description");
    let r = m
        .enviar("POST", "/api/v1/companies", Some(&ana.token), Some(cuerpo))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    assert_eq!(r.json["name"], "Café Nuevo");
    for k in ["website", "phone", "description"] {
        assert_eq!(r.json[k], Value::Null, "{k}");
    }
    m.db.cleanup().await;
}

/// Un campo fuera del contrato es 422 `campo_no_permitido`, con o sin nombre conocido, y no
/// se guarda nada. Incluye los que intentarían darse un rol o verificarse (RO6).
#[tokio::test]
async fn los_cuerpos_son_cerrados_y_nadie_se_da_un_rol() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    for extra in [
        ("verified_at", json!("2026-01-01T00:00:00Z")),
        ("verified", json!(true)),
        ("role", json!("admin")),
        ("roles", json!(["admin"])),
        ("my_role", json!("company_admin")),
        ("id", json!("00000000-0000-7000-8000-000000000000")),
        ("membership", json!({"type": "business"})),
        ("created_by", json!("x")),
    ] {
        let mut c = m.cuerpo_empresa(NIT, "Café").await;
        c[extra.0] = extra.1;
        let r = m
            .enviar("POST", "/api/v1/companies", Some(&ana.token), Some(c))
            .await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", extra.0);
        assert_eq!(r.json["error"], "campo_no_permitido", "{}", extra.0);
        assert_eq!(
            r.json["message"],
            "El cuerpo trae un campo que no se admite."
        );
    }
    assert_eq!(m.n("SELECT count(*) FROM companies").await, 0);
    assert_eq!(m.n("SELECT count(*) FROM user_roles ur JOIN roles r ON r.id = ur.role_id WHERE r.code <> 'applicant'").await, 0);
    m.db.cleanup().await;
}

#[tokio::test]
async fn un_cuerpo_que_no_es_un_objeto_json_es_422_con_texto_fijo() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    for crudo in [&b"no es json {"[..], b"[1,2]", b"\"hola\"", b"null", b""] {
        let r = m
            .enviar_crudo(
                "POST",
                "/api/v1/companies",
                Some(&ana.token),
                crudo.to_vec(),
                &[],
            )
            .await;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{:?}",
            String::from_utf8_lossy(crudo)
        );
        assert_eq!(r.json["error"], "datos_invalidos");
    }
    m.db.cleanup().await;
}

/// El tope de cuerpo de `POST`/`PUT /company` es 16 KB: 12 KB llega a la validación (422 por la
/// descripción) y 17 KB es 413.
#[tokio::test]
async fn el_tope_de_cuerpo_es_16_kb() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let base = m.cuerpo_empresa(NIT, "Café").await;
    let con_relleno = |n: usize| {
        let mut c = base.clone();
        c["description"] = json!("d".repeat(n));
        c.to_string().into_bytes()
    };
    for (metodo, ruta) in [("POST", "/api/v1/companies"), ("PUT", "/api/v1/company")] {
        let r = m
            .enviar_crudo(metodo, ruta, Some(&ana.token), con_relleno(12_000), &[])
            .await;
        // PUT sin permiso responde antes que el tamaño: 403. POST llega a validar: 422.
        let esperado = if metodo == "POST" {
            StatusCode::UNPROCESSABLE_ENTITY
        } else {
            StatusCode::FORBIDDEN
        };
        assert_eq!(r.status, esperado, "{metodo} 12 KB");
        let r = m
            .enviar_crudo(metodo, ruta, Some(&ana.token), con_relleno(17_000), &[])
            .await;
        let esperado = if metodo == "POST" {
            StatusCode::PAYLOAD_TOO_LARGE
        } else {
            StatusCode::FORBIDDEN
        };
        assert_eq!(r.status, esperado, "{metodo} 17 KB");
    }
    // Con permiso (admin de una empresa), el PUT sí llega al tamaño.
    m.crear_empresa(&ana, NIT, "Café").await;
    let r = m
        .enviar_crudo(
            "PUT",
            "/api/v1/company",
            Some(&ana.token),
            con_relleno(12_000),
            &[],
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = m
        .enviar_crudo(
            "PUT",
            "/api/v1/company",
            Some(&ana.token),
            con_relleno(17_000),
            &[],
        )
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    m.db.cleanup().await;
}

// ------------------------------------------------------ SX3 y límites

#[tokio::test]
async fn sx3_sin_token_todo_es_401() {
    let m = mundo().await;
    let id = "00000000-0000-7000-8000-000000000000";
    for (metodo, ruta) in [
        ("POST", "/api/v1/companies".to_string()),
        ("GET", "/api/v1/company".into()),
        ("PUT", "/api/v1/company".into()),
        ("GET", "/api/v1/company/recruiters".into()),
        ("DELETE", format!("/api/v1/company/recruiters/{id}")),
        ("POST", "/api/v1/company/invitations".into()),
        ("GET", "/api/v1/company/invitations".into()),
        ("DELETE", format!("/api/v1/company/invitations/{id}")),
        ("POST", "/api/v1/invitations/accept".into()),
        ("GET", "/api/v1/admin/companies".into()),
        ("POST", format!("/api/v1/admin/companies/{id}/verify")),
        ("POST", format!("/api/v1/admin/companies/{id}/unverify")),
        ("PUT", format!("/api/v1/admin/companies/{id}/membership")),
    ] {
        let r = m.enviar(metodo, &ruta, None, Some(json!({}))).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{metodo} {ruta}");
    }
    m.db.cleanup().await;
}

#[tokio::test]
async fn crear_empresas_esta_limitado_a_5_por_persona_y_dia() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let beto = m.persona("beto").await;
    for i in 0..5 {
        m.crear_empresa(&ana, &format!("100{i}-1"), &format!("E{i}"))
            .await;
        // Sale de la empresa por la base (la API no deja salir al único administrador).
        m.db.exec(&format!(
            "UPDATE company_recruiters SET deleted_at = now() WHERE user_id = '{}'",
            ana.id
        ))
        .await
        .unwrap();
    }
    let r = m
        .enviar(
            "POST",
            "/api/v1/companies",
            Some(&ana.token),
            Some(m.cuerpo_empresa("2000-1", "Seis").await),
        )
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.text);
    assert_eq!(r.json["error"], "demasiados_intentos");
    let espera: u64 = r.headers["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=86_400).contains(&espera));
    assert_eq!(m.n("SELECT count(*) FROM companies").await, 5);
    // Es por persona: otra no se ve afectada.
    m.crear_empresa(&beto, "3000-1", "Beto").await;
    // Pasado el día (se mueve el registro, no el reloj) vuelve a poder.
    m.db.exec("UPDATE rate_events SET created_at = created_at - interval '2 days'")
        .await
        .unwrap();
    m.crear_empresa(&ana, "2000-1", "Seis").await;
    m.db.cleanup().await;
}

// ------------------------------------------------------ GET y PUT /company

#[tokio::test]
async fn get_company_devuelve_la_mia_y_sin_empresa_es_404() {
    let m = mundo().await;
    let (ana, id) = m.admin_con_empresa("ana", NIT).await;
    let (beto, id_b) = m.admin_con_empresa("beto", "7654321-0").await;
    let r = m.get("/api/v1/company", &ana).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["id"], id);
    assert_eq!(r.json["tax_id"], NIT);
    assert_eq!(r.json["my_role"], "company_admin");
    let r = m.get("/api/v1/company", &beto).await;
    assert_eq!(r.json["id"], id_b);
    assert_ne!(id, id_b);
    // Sin empresa: 404 genérico (también quien recibe el rol por la base: un rol sin fila no sirve).
    let carla = m.persona("carla").await;
    let r = m.get("/api/v1/company", &carla).await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("no_encontrado"))
    );
    m.dar_rol(&carla.id, "company_admin").await;
    let r2 = m.get("/api/v1/company", &carla).await;
    assert_eq!(
        (r2.status, r2.json.clone()),
        (StatusCode::NOT_FOUND, r.json.clone())
    );
    let r3 = m.get("/api/v1/company/recruiters", &carla).await;
    assert_eq!((r3.status, r3.json), (StatusCode::NOT_FOUND, r.json));
    m.db.cleanup().await;
}

#[tokio::test]
async fn put_company_reemplaza_es_idempotente_y_exige_el_permiso() {
    let m = mundo().await;
    let (ana, _) = m.admin_con_empresa("ana", NIT).await;
    let mut nuevo = m.cuerpo_empresa(NIT, "Café Renovado").await;
    nuevo["website"] = Value::Null;
    nuevo["description"] = json!("Otra descripción");
    for _ in 0..2 {
        let r = m
            .enviar(
                "PUT",
                "/api/v1/company",
                Some(&ana.token),
                Some(nuevo.clone()),
            )
            .await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
        assert!(r.text.is_empty());
    }
    let c = m.get("/api/v1/company", &ana).await.json;
    assert_eq!(c["name"], "Café Renovado");
    assert_eq!(c["website"], Value::Null);
    assert_eq!(c["description"], "Otra descripción");
    // Un reclutador y un candidato: 403 sin_permiso, aunque el cuerpo sea inválido.
    let rec = m.miembro(&ana, "rec", "recruiter").await;
    let cand = m.persona("cand").await;
    for p in [&rec, &cand] {
        let r = m
            .enviar(
                "PUT",
                "/api/v1/company",
                Some(&p.token),
                Some(json!({"x": 1})),
            )
            .await;
        assert_eq!(
            (r.status, r.json["error"].as_str()),
            (StatusCode::FORBIDDEN, Some("sin_permiso"))
        );
    }
    // El reclutador sí ve la empresa, con su rol.
    let r = m.get("/api/v1/company", &rec).await;
    assert_eq!(r.json["my_role"], "recruiter");
    // Validación y campos cerrados también en el PUT.
    let r = m
        .enviar(
            "PUT",
            "/api/v1/company",
            Some(&ana.token),
            Some(json!({"name": "x"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let mut c = m.cuerpo_empresa(NIT, "Café").await;
    c["verified_at"] = json!("2026-01-01T00:00:00Z");
    let r = m
        .enviar("PUT", "/api/v1/company", Some(&ana.token), Some(c))
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("campo_no_permitido"))
    );
    assert_eq!(m.get("/api/v1/company", &ana).await.json["verified"], false);
    m.db.cleanup().await;
}

#[tokio::test]
async fn put_company_con_el_permiso_pero_sin_empresa_es_404() {
    let m = mundo().await;
    let carla = m.persona("carla").await;
    m.dar_rol(&carla.id, "company_admin").await;
    let r = m
        .enviar(
            "PUT",
            "/api/v1/company",
            Some(&carla.token),
            Some(m.cuerpo_empresa(NIT, "Café").await),
        )
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("no_encontrado"))
    );
    m.db.cleanup().await;
}

#[tokio::test]
async fn el_nit_de_una_empresa_verificada_es_inmutable_y_el_de_una_no_verificada_no_puede_chocar() {
    let m = mundo().await;
    let (ana, id) = m.admin_con_empresa("ana", NIT).await;
    let (_beto, _) = m.admin_con_empresa("beto", "7654321-0").await;
    // Sin verificar: cambiar el NIT a uno libre vale; a uno ajeno, 409 duplicada.
    let r = m
        .enviar(
            "PUT",
            "/api/v1/company",
            Some(&ana.token),
            Some(m.cuerpo_empresa("7654321-0", "Café").await),
        )
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::CONFLICT, Some("empresa_duplicada"))
    );
    let r = m
        .enviar(
            "PUT",
            "/api/v1/company",
            Some(&ana.token),
            Some(m.cuerpo_empresa("5555555-5", "Café").await),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        m.get("/api/v1/company", &ana).await.json["tax_id"],
        "5555555-5"
    );
    // Verificada: otro NIT → 409 nit_inmutable; el mismo con otro formato, vale; el nombre cambia
    // y la verificación se queda.
    m.db.exec(&format!(
        "UPDATE companies SET verified_at = now() WHERE id = '{id}'"
    ))
    .await
    .unwrap();
    let antes = m
        .texto(&format!(
            "SELECT verified_at::text FROM companies WHERE id = '{id}'"
        ))
        .await;
    let r = m
        .enviar(
            "PUT",
            "/api/v1/company",
            Some(&ana.token),
            Some(m.cuerpo_empresa("6666666-6", "Café").await),
        )
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::CONFLICT, Some("nit_inmutable"))
    );
    let r = m
        .enviar(
            "PUT",
            "/api/v1/company",
            Some(&ana.token),
            Some(m.cuerpo_empresa("55555555", "Nombre nuevo").await),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let c = m.get("/api/v1/company", &ana).await.json;
    assert_eq!(c["name"], "Nombre nuevo");
    assert_eq!(c["tax_id"], "55555555");
    assert_eq!(c["verified"], true);
    let despues = m
        .texto(&format!(
            "SELECT verified_at::text FROM companies WHERE id = '{id}'"
        ))
        .await;
    assert_eq!(antes, despues);
    m.db.cleanup().await;
}

// ------------------------------------------------------------ aislamiento

/// SX2/SX6: un recurso de otra empresa, uno inexistente y un id mal formado dan el mismo 404.
#[tokio::test]
async fn sx6_lo_ajeno_inexistente_y_mal_formado_es_el_mismo_404() {
    let m = mundo().await;
    let (ana, _) = m.admin_con_empresa("ana", NIT).await;
    let (beto, _) = m.admin_con_empresa("beto", "7654321-0").await;
    let rec_b = m.miembro(&beto, "recb", "recruiter").await;
    let id_ajeno: String = m
        .texto(&format!(
            "SELECT id::text FROM company_recruiters WHERE user_id = '{}'",
            rec_b.id
        ))
        .await;
    let inv_ajena = m
        .invitar(&beto, "otro@correo-prueba.com", "recruiter")
        .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    for (base, ajeno) in [("recruiters", &id_ajeno), ("invitations", &inv_ajena)] {
        let mut vistos: Vec<(StatusCode, Value)> = vec![];
        for id in [
            ajeno.as_str(),
            "00000000-0000-7000-8000-000000000000",
            "no-es-uuid",
            "123",
        ] {
            let r = m
                .enviar(
                    "DELETE",
                    &format!("/api/v1/company/{base}/{id}"),
                    Some(&ana.token),
                    None,
                )
                .await;
            vistos.push((r.status, r.json));
        }
        assert_eq!(vistos[0].0, StatusCode::NOT_FOUND, "{base}");
        assert_eq!(vistos[0].1["error"], "no_encontrado");
        assert!(vistos.iter().all(|v| v == &vistos[0]), "{base}: {vistos:?}");
    }
    // Y nada cambió en la empresa de Beto.
    assert_eq!(
        m.n("SELECT count(*) FROM company_recruiters WHERE deleted_at IS NULL")
            .await,
        3
    );
    assert_eq!(
        m.n("SELECT count(*) FROM company_invitations WHERE revoked_at IS NULL")
            .await,
        2
    );
    m.db.cleanup().await;
}

// ---------------------------------------------------- reclutadores

#[tokio::test]
async fn em4_solo_el_administrador_lista_y_quita_reclutadores() {
    let m = mundo().await;
    let (ana, _) = m.admin_con_empresa("ana", NIT).await;
    let rec = m.miembro(&ana, "rec", "recruiter").await;
    let cand = m.persona("cand").await;
    for p in [&rec, &cand] {
        for (metodo, ruta) in [
            ("GET", "/api/v1/company/recruiters".to_string()),
            (
                "DELETE",
                "/api/v1/company/recruiters/00000000-0000-7000-8000-000000000000".into(),
            ),
            ("POST", "/api/v1/company/invitations".into()),
            ("GET", "/api/v1/company/invitations".into()),
            (
                "DELETE",
                "/api/v1/company/invitations/00000000-0000-7000-8000-000000000000".into(),
            ),
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
    m.db.cleanup().await;
}

#[tokio::test]
async fn el_listado_de_reclutadores_solo_trae_nombre_corto_y_pagina() {
    let m = mundo().await;
    let (ana, _) = m.admin_con_empresa("ana", NIT).await;
    let r1 = m.miembro(&ana, "uno", "recruiter").await;
    let _r2 = m.miembro(&ana, "dos", "recruiter").await;
    m.db.exec(&format!(
        "UPDATE users SET married_last_name = 'MARCA-CASADA', middle_name = 'MARCA-MEDIO', \
         last_name2 = 'MARCA-SEGUNDO' WHERE id = '{}'",
        r1.id
    ))
    .await
    .unwrap();
    let (_beto, _) = m.admin_con_empresa("beto", "7654321-0").await;
    let r = m.get("/api/v1/company/recruiters", &ana).await;
    assert_eq!(r.status, StatusCode::OK);
    let items = r.json["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(r.json["next_cursor"], Value::Null);
    for i in items {
        let mut k: Vec<&String> = i.as_object().unwrap().keys().collect();
        k.sort();
        assert_eq!(k, ["display_name", "id", "joined_at", "position", "role"]);
        assert_eq!(i["display_name"], "Ana Prueba");
    }
    assert_eq!(items[0]["role"], "company_admin");
    for prohibido in ["correo-prueba", "@", "MARCA-", "married"] {
        assert!(!r.text.contains(prohibido), "{prohibido}: {}", r.text);
    }
    // Paginación: de a uno, sin repetir ni saltar.
    let mut vistos = vec![];
    let mut cursor: Option<String> = None;
    loop {
        let url = match &cursor {
            None => "/api/v1/company/recruiters?limit=1".to_string(),
            Some(c) => format!("/api/v1/company/recruiters?limit=1&cursor={c}"),
        };
        let p = m.get(&url, &ana).await.json;
        for i in p["items"].as_array().unwrap() {
            vistos.push(i["id"].as_str().unwrap().to_string());
        }
        match p["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    assert_eq!(vistos.len(), 3);
    let mut unicos = vistos.clone();
    unicos.sort();
    unicos.dedup();
    assert_eq!(unicos.len(), 3);
    for malo in [
        "limit=0",
        "limit=51",
        "limit=abc",
        "cursor=%25%25",
        "cursor=aGVsbG8",
    ] {
        let r = m
            .get(&format!("/api/v1/company/recruiters?{malo}"), &ana)
            .await;
        assert_eq!(
            (r.status, r.json["error"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some("datos_invalidos")),
            "{malo}"
        );
    }
    let r = m.get("/api/v1/company/recruiters?name=ana", &ana).await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("parametro_no_permitido")
        )
    );
    assert!(!r.text.contains("name"));
    m.db.cleanup().await;
}

/// EM9: quitar a un reclutador no borra la empresa; le quita los roles de empresa en la petición
/// siguiente y no toca su cuenta de candidato. EM8: la misma cuenta sigue siendo candidato.
#[tokio::test]
async fn em9_quitar_a_un_reclutador_conserva_la_empresa_y_la_cuenta() {
    let m = mundo().await;
    let (ana, id) = m.admin_con_empresa("ana", NIT).await;
    let rec = m.miembro(&ana, "rec", "recruiter").await;
    let fila = m
        .texto(&format!(
            "SELECT id::text FROM company_recruiters WHERE user_id = '{}'",
            rec.id
        ))
        .await;
    assert_eq!(
        m.get("/api/v1/me", &rec).await.json["roles"],
        json!(["applicant", "recruiter"])
    );
    assert_eq!(m.get("/api/v1/company", &rec).await.status, StatusCode::OK);
    let r = m
        .enviar(
            "DELETE",
            &format!("/api/v1/company/recruiters/{fila}"),
            Some(&ana.token),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    // Efecto inmediato: su token sigue valiendo para su cuenta, no para la empresa.
    let me = m.get("/api/v1/me", &rec).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.json["roles"], json!(["applicant"]));
    assert_eq!(me.json["permissions"], json!([]));
    assert_eq!(
        m.get("/api/v1/company", &rec).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(m.get("/api/v1/profile", &rec).await.status, StatusCode::OK);
    // La empresa, intacta; la fila queda con `deleted_at`.
    assert_eq!(
        m.n(&format!(
            "SELECT count(*) FROM companies WHERE id = '{id}' AND deleted_at IS NULL"
        ))
        .await,
        1
    );
    assert_eq!(
        m.n("SELECT count(*) FROM company_recruiters WHERE deleted_at IS NOT NULL")
            .await,
        1
    );
    // Borrar dos veces: la segunda ya no existe.
    let r = m
        .enviar(
            "DELETE",
            &format!("/api/v1/company/recruiters/{fila}"),
            Some(&ana.token),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // Quien salió puede crear su propia empresa o entrar a otra.
    m.crear_empresa(&rec, "9999999-9", "La mía").await;
    m.db.cleanup().await;
}

#[tokio::test]
async fn em9_el_ultimo_administrador_no_se_puede_quitar_ni_salir() {
    let m = mundo().await;
    let (ana, _) = m.admin_con_empresa("ana", NIT).await;
    let propia = m
        .texto(&format!(
            "SELECT id::text FROM company_recruiters WHERE user_id = '{}'",
            ana.id
        ))
        .await;
    let r = m
        .enviar(
            "DELETE",
            &format!("/api/v1/company/recruiters/{propia}"),
            Some(&ana.token),
            None,
        )
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::CONFLICT, Some("ultimo_administrador"))
    );
    assert_eq!(m.get("/api/v1/company", &ana).await.status, StatusCode::OK);
    // Con un segundo administrador, el primero puede salir; el segundo, ya único, no.
    let otro = m.miembro(&ana, "otro", "company_admin").await;
    let r = m
        .enviar(
            "DELETE",
            &format!("/api/v1/company/recruiters/{propia}"),
            Some(&ana.token),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        m.get("/api/v1/company", &ana).await.status,
        StatusCode::NOT_FOUND
    );
    let suya = m
        .texto(&format!(
            "SELECT id::text FROM company_recruiters WHERE user_id = '{}' AND deleted_at IS NULL",
            otro.id
        ))
        .await;
    let r = m
        .enviar(
            "DELETE",
            &format!("/api/v1/company/recruiters/{suya}"),
            Some(&otro.token),
            None,
        )
        .await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::CONFLICT, Some("ultimo_administrador"))
    );
    m.db.cleanup().await;
}

#[tokio::test]
async fn em9_borrar_la_cuenta_respeta_al_ultimo_administrador() {
    let m = mundo().await;
    let (ana, id) = m.admin_con_empresa("ana", NIT).await;
    let rec = m.miembro(&ana, "rec", "recruiter").await;
    // El único administrador no puede borrarse mientras exista la empresa; no se borra nada.
    let r = borrar(&m, &ana).await;
    assert_eq!(
        (r.status, r.json["error"].as_str()),
        (StatusCode::CONFLICT, Some("ultimo_administrador"))
    );
    assert_eq!(m.n("SELECT count(*) FROM users").await, 2);
    assert_eq!(m.n("SELECT count(*) FROM privacy_events").await, 0);
    // Un reclutador sí: la empresa y el resto quedan; su fila de reclutador se va.
    let r = borrar(&m, &rec).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    assert_eq!(
        m.n(&format!("SELECT count(*) FROM companies WHERE id = '{id}'"))
            .await,
        1
    );
    assert_eq!(
        m.n("SELECT count(*) FROM company_recruiters WHERE deleted_at IS NULL")
            .await,
        1
    );
    // Con otro administrador, el primero puede borrarse.
    let otro = m.miembro(&ana, "otro", "company_admin").await;
    let r = borrar(&m, &ana).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    assert_eq!(m.get("/api/v1/company", &otro).await.status, StatusCode::OK);
    m.db.cleanup().await;
}

/// EM8: una persona es candidato y reclutador a la vez, y los datos de uno no salen en las
/// respuestas del otro.
#[tokio::test]
async fn em8_candidato_y_reclutador_a_la_vez_sin_mezclar_respuestas() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    m.crear_empresa(&ana, NIT, "Café").await;
    let me = m.get("/api/v1/me", &ana).await.json;
    assert_eq!(me["roles"], json!(["applicant", "company_admin"]));
    assert_eq!(m.get("/api/v1/profile", &ana).await.status, StatusCode::OK);
    assert_eq!(m.get("/api/v1/cvs", &ana).await.status, StatusCode::OK);
    // La respuesta de la empresa no lleva nada de la cuenta de candidato.
    let c = m.get("/api/v1/company", &ana).await;
    for prohibido in ["correo-prueba", "applicant", "first_name", "email"] {
        assert!(!c.text.contains(prohibido), "{prohibido}: {}", c.text);
    }
    m.db.cleanup().await;
}

async fn borrar(m: &Mundo, p: &empresas::Persona) -> empresas::Resp {
    m.enviar(
        "DELETE",
        "/api/v1/me",
        Some(&p.token),
        Some(json!({"password": empresas::CLAVE})),
    )
    .await
}

// ---------------------------------------------------------- concurrencia

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dos_creaciones_simultaneas_de_una_persona_dejan_una_empresa() {
    let m = mundo().await;
    let ana = m.persona("ana").await;
    let (a, b) = (
        m.cuerpo_empresa("1111111-1", "Uno").await,
        m.cuerpo_empresa("2222222-2", "Dos").await,
    );
    let (r1, r2) = tokio::join!(
        m.enviar("POST", "/api/v1/companies", Some(&ana.token), Some(a)),
        m.enviar("POST", "/api/v1/companies", Some(&ana.token), Some(b)),
    );
    let mut estados = [r1.status, r2.status];
    estados.sort();
    assert_eq!(
        estados,
        [StatusCode::CREATED, StatusCode::CONFLICT],
        "{} / {}",
        r1.text,
        r2.text
    );
    let perdedor = if r1.status == StatusCode::CONFLICT {
        &r1
    } else {
        &r2
    };
    assert_eq!(perdedor.json["error"], "ya_tiene_empresa");
    assert_eq!(m.n("SELECT count(*) FROM companies").await, 1);
    assert_eq!(m.n("SELECT count(*) FROM company_recruiters").await, 1);
    m.db.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dos_personas_con_el_mismo_nit_a_la_vez_dejan_una_empresa() {
    let m = mundo().await;
    let (ana, beto) = (m.persona("ana").await, m.persona("beto").await);
    let (a, b) = (
        m.cuerpo_empresa(NIT, "Uno").await,
        m.cuerpo_empresa("1234567 8", "Dos").await,
    );
    let (r1, r2) = tokio::join!(
        m.enviar("POST", "/api/v1/companies", Some(&ana.token), Some(a)),
        m.enviar("POST", "/api/v1/companies", Some(&beto.token), Some(b)),
    );
    let mut estados = [r1.status, r2.status];
    estados.sort();
    assert_eq!(estados, [StatusCode::CREATED, StatusCode::CONFLICT]);
    assert_eq!(m.n("SELECT count(*) FROM companies").await, 1);
    // La perdedora no queda a medias: sin fila de reclutador ni rol.
    assert_eq!(m.n("SELECT count(*) FROM company_recruiters").await, 1);
    assert_eq!(m.n("SELECT count(*) FROM user_roles ur JOIN roles r ON r.id = ur.role_id WHERE r.code = 'company_admin'").await, 1);
    m.db.cleanup().await;
}

// ------------------------------------------------------------------- logs

/// Ni el NIT, ni el nombre de la empresa, ni el correo, ni el token salen en un log o en un
/// error, ni siquiera cuando algo falla por dentro.
#[tokio::test]
async fn el_log_y_los_errores_no_llevan_nit_nombre_ni_correo() {
    let (log, _guarda) = capturar_log();
    let m = mundo().await;
    let ana = m
        .persona_con_email("marca.correo.ana@correo-prueba.com")
        .await;
    let beto = m
        .persona_con_email("marca.correo.beto@correo-prueba.com")
        .await;
    let nit = "MARCA-NIT-7731";
    let nombre = "MARCA Empresa Secreta";
    let mut respuestas: Vec<String> = vec![];
    m.crear_empresa(&ana, nit, nombre).await;
    for (p, cuerpo) in [
        (&beto, m.cuerpo_empresa(nit, "Otra").await),
        (&ana, m.cuerpo_empresa("OTRO-NIT-9", nombre).await),
        (&beto, json!({"name": nombre, "tax_id": nit, "extra": 1})),
        (&beto, json!({"name": nombre, "tax_id": "x"})),
    ] {
        let r = m
            .enviar("POST", "/api/v1/companies", Some(&p.token), Some(cuerpo))
            .await;
        assert!(r.status.is_client_error());
        respuestas.push(r.text);
    }
    // Un fallo interno: la tabla desaparece a mitad.
    m.db.exec("ALTER TABLE company_recruiters RENAME TO company_recruiters_rota")
        .await
        .unwrap();
    let r = m.get("/api/v1/company", &ana).await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
    respuestas.push(r.text);
    let r = m
        .enviar(
            "POST",
            "/api/v1/companies",
            Some(&beto.token),
            Some(m.cuerpo_empresa("OTRO-NIT-5", nombre).await),
        )
        .await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
    respuestas.push(r.text);

    let texto = log.texto();
    assert!(texto.contains("captura-activa"), "no se capturó ningún log");
    respuestas.push(texto);
    for fuente in respuestas {
        for secreto in [
            nit,
            "7731",
            "MARCA",
            "OTRO-NIT",
            "marca.correo",
            ana.token.as_str(),
            beto.token.as_str(),
        ] {
            assert!(!fuente.contains(secreto), "se filtró {secreto:?}: {fuente}");
        }
    }
    m.db.cleanup().await;
}

/// Cordura de la prueba anterior: el mismo mecanismo SÍ ve un secreto que alguien loguee.
#[tokio::test]
async fn la_captura_de_log_ve_lo_que_se_escribe() {
    let (log, _guarda) = capturar_log();
    tracing::warn!(nit = "MARCA-NIT-7731", "esto sí se vería");
    assert!(log.texto().contains("MARCA-NIT-7731"));
}

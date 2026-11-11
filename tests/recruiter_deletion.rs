//! Casos de verdad del borrado de la cuenta de un reclutador (Fase 5a, paso 10: BO3, BO4), contra
//! «Borrado y retención» de `api.md`. PostgreSQL real, una base por prueba.
//!
//! Lo del candidato (BO1, BO2, PA10) ya lo fijan `tests/applications.rs` y `tests/panel.rs`.

mod common;
#[path = "common/empresas.rs"]
mod empresas;

use axum::http::StatusCode;
use empresas::{mundo, Mundo, Persona, Resp, CLAVE};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::json;

const MINIMO: [&str; 3] = ["display_name", "skills", "experience"];
const NOTA: &str = "NOTA-DEL-RECLUTADOR-QUE-SE-VA";

struct Esc {
    m: Mundo,
    admin: Persona,
    rec: Persona,
    ana: Persona,
    emp: String,
    oferta: String,
    app: String,
}

/// Empresa verificada y vigente creada por `admin` por la API; `rec` entra por invitación, crea
/// y publica la oferta, y decide sobre la aplicación de Ana con una nota.
async fn escenario() -> Esc {
    let m = mundo().await;
    let (admin, emp) = m.admin_con_empresa("admin", "1234567-8").await;
    let plataforma = m.admin("plataforma").await;
    let r = m
        .enviar(
            "POST",
            &format!("/api/v1/admin/companies/{emp}/verify"),
            Some(&plataforma.token),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    let r = m
        .enviar(
            "PUT",
            &format!("/api/v1/admin/companies/{emp}/membership"),
            Some(&plataforma.token),
            Some(json!({"membership_type": "business",
                        "start_date": m.texto("SELECT (current_date - 1)::text").await,
                        "expiration_date": m.texto("SELECT (current_date + 100)::text").await})),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    let rec = m.miembro(&admin, "rec", "recruiter").await;

    let oferta = m.texto("SELECT gen_random_uuid()::text").await;
    m.db.exec(&format!(
        "INSERT INTO job_offers (id, company_id, created_by, name, description, status, publication_date) \
         VALUES ('{oferta}', '{emp}', \
                 (SELECT id FROM company_recruiters WHERE user_id = '{}'), \
                 'Barista', 'Una oferta.', 'published', current_date)",
        rec.id
    ))
    .await
    .unwrap();

    let ana = m.persona("ana").await;
    let r = m
        .enviar(
            "POST",
            &format!("/api/v1/offers/{oferta}/applications"),
            Some(&ana.token),
            Some(json!({"consent": true, "shared_fields": MINIMO})),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    let app = r.json["id"].as_str().unwrap().to_string();
    let r = m
        .enviar(
            "POST",
            &format!("/api/v1/company/applications/{app}/status"),
            Some(&rec.token),
            Some(json!({"to": "under_review", "note": NOTA})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    Esc {
        m,
        admin,
        rec,
        ana,
        emp,
        oferta,
        app,
    }
}

async fn borrar(m: &Mundo, p: &Persona) -> Resp {
    m.enviar(
        "DELETE",
        "/api/v1/me",
        Some(&p.token),
        Some(json!({"password": CLAVE})),
    )
    .await
}

/// Todas las columnas `uuid` de la base que guardan **quién** hizo algo (`*_by`).
async fn columnas_de_autoria(m: &Mundo) -> Vec<(String, String)> {
    m.db.conn
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT table_name::text, column_name::text FROM information_schema.columns \
             WHERE table_schema = 'public' AND data_type = 'uuid' AND column_name LIKE '%\\_by' \
             ORDER BY 1, 2"
                .to_string(),
        ))
        .await
        .unwrap()
        .iter()
        .map(|f| (f.try_get_by_index(0).unwrap(), f.try_get_by_index(1).unwrap()))
        .collect()
}

/// BO3: borrar la cuenta del reclutador conserva sus decisiones como anónimas: el evento queda
/// (con su nota interna) sin `changed_by`, la oferta queda sin `created_by`, la empresa y el panel
/// siguen; su membresía y sus roles se van.
#[tokio::test]
async fn bo3_las_decisiones_del_reclutador_quedan_anonimas() {
    let e = escenario().await;
    let r = borrar(&e.m, &e.rec).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);

    assert_eq!(
        e.m.n(&format!("SELECT count(*) FROM users WHERE id = '{}'", e.rec.id))
            .await,
        0
    );
    assert_eq!(
        e.m.n(&format!(
            "SELECT count(*) FROM company_recruiters WHERE user_id = '{}'",
            e.rec.id
        ))
        .await,
        0
    );
    assert_eq!(
        e.m.n(&format!(
            "SELECT count(*) FROM user_roles WHERE user_id = '{}'",
            e.rec.id
        ))
        .await,
        0
    );
    // El evento sigue, sin quién, y con la nota de la empresa.
    assert_eq!(
        e.m.n(&format!(
            "SELECT count(*) FROM job_application_events \
             WHERE job_application_id = '{}' AND to_status = 'under_review' \
               AND changed_by IS NULL AND note IS NOT NULL",
            e.app
        ))
        .await,
        1
    );
    // La oferta sigue, sin autor.
    assert_eq!(
        e.m.n(&format!(
            "SELECT count(*) FROM job_offers WHERE id = '{}' AND created_by IS NULL AND company_id = '{}'",
            e.oferta, e.emp
        ))
        .await,
        1
    );
    let r = e
        .m
        .get(&format!("/api/v1/company/offers/{}", e.oferta), &e.admin)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json["created_by"], serde_json::Value::Null);
    // El panel de la empresa sigue entero; el historial no dice quién decidió.
    let r = e
        .m
        .get(&format!("/api/v1/company/applications/{}", e.app), &e.admin)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json["status"], "under_review");
    assert_eq!(r.json["history"][1]["note"], NOTA);
    assert!(!r.text.contains(&e.rec.id));
    assert!(!r.text.contains(&e.rec.email));
    // La persona que aplicó no pierde nada.
    let r = e
        .m
        .get(&format!("/api/v1/me/applications/{}", e.app), &e.ana)
        .await;
    assert_eq!(r.status, StatusCode::OK);
    e.m.db.cleanup().await;
}

/// BO3: ninguna columna de autoría (`*_by`) de ninguna tabla conserva el id de la cuenta borrada,
/// y el rastro de auditoría queda sin actor.
#[tokio::test]
async fn bo3_ninguna_columna_de_autoria_conserva_su_id() {
    let e = escenario().await;
    // El reclutador también deja rastro en la empresa y en la auditoría del panel.
    assert_eq!(
        e.m.get(&format!("/api/v1/company/applications/{}", e.app), &e.rec)
            .await
            .status,
        StatusCode::OK
    );
    let antes = e
        .m
        .n(&format!(
            "SELECT count(*) FROM access_audit WHERE actor_user_id = '{}'",
            e.rec.id
        ))
        .await;
    assert!(antes >= 2, "{antes}");
    let r = borrar(&e.m, &e.rec).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);

    let columnas = columnas_de_autoria(&e.m).await;
    assert!(columnas.len() > 5, "{columnas:?}");
    for (tabla, col) in &columnas {
        let n = e
            .m
            .n(&format!(
                "SELECT count(*) FROM \"{tabla}\" WHERE \"{col}\" = '{}'",
                e.rec.id
            ))
            .await;
        assert_eq!(n, 0, "{tabla}.{col} conserva el id de la cuenta borrada");
    }
    assert_eq!(
        e.m.n("SELECT count(*) FROM access_audit WHERE actor_user_id IS NULL")
            .await,
        antes
    );
    e.m.db.cleanup().await;
}

/// BO3, lo mismo para quien administra: el admin de la empresa que no es el único se puede ir,
/// y lo que hizo sobre la empresa (editarla, invitar) queda sin autor.
#[tokio::test]
async fn bo3_un_administrador_que_no_es_el_unico_se_va_sin_dejar_su_id() {
    let e = escenario().await;
    let otro = e.m.miembro(&e.admin, "otro-admin", "company_admin").await;
    let r = e
        .m
        .enviar(
            "PUT",
            "/api/v1/company",
            Some(&e.admin.token),
            Some(e.m.cuerpo_empresa("1234567-8", "Empresa renombrada").await),
        )
        .await;
    assert!(r.status.is_success(), "{}", r.text);
    let r = borrar(&e.m, &e.admin).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text);
    for (tabla, col) in columnas_de_autoria(&e.m).await {
        let n = e
            .m
            .n(&format!(
                "SELECT count(*) FROM \"{tabla}\" WHERE \"{col}\" = '{}'",
                e.admin.id
            ))
            .await;
        assert_eq!(n, 0, "{tabla}.{col} conserva el id del administrador borrado");
    }
    // La empresa sigue con su otro administrador.
    let r = e.m.get("/api/v1/company", &otro).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    e.m.db.cleanup().await;
}

/// BO4: la constancia es la misma fila `account_deleted`, sin nombre, correo ni id.
#[tokio::test]
async fn bo4_la_constancia_no_tiene_datos_personales() {
    let e = escenario().await;
    assert_eq!(
        borrar(&e.m, &e.rec).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        e.m.n("SELECT count(*) FROM privacy_events").await,
        1
    );
    assert_eq!(
        e.m.n("SELECT count(*) FROM privacy_events WHERE kind = 'account_deleted' AND notes IS NULL")
            .await,
        1
    );
    let fila = e
        .m
        .texto("SELECT row_to_json(p)::text FROM privacy_events p")
        .await;
    for prohibido in [e.rec.id.as_str(), e.rec.email.as_str(), "Ana", "Prueba"] {
        assert!(!fila.contains(prohibido), "la constancia trae {prohibido:?}");
    }
    e.m.db.cleanup().await;
}

/// EM9 sigue en pie: el único administrador no se puede borrar y no se borra nada.
#[tokio::test]
async fn el_unico_administrador_no_se_borra() {
    let e = escenario().await;
    let r = borrar(&e.m, &e.admin).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text);
    assert_eq!(r.json["error"], "ultimo_administrador");
    assert_eq!(
        e.m.n(&format!("SELECT count(*) FROM users WHERE id = '{}'", e.admin.id))
            .await,
        1
    );
    assert_eq!(e.m.n("SELECT count(*) FROM privacy_events").await, 0);
    e.m.db.cleanup().await;
}

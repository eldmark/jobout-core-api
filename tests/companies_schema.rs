//! Esquema de empresas (Fase 5a, paso 3): EM10 y las garantías que la base debe dar sola,
//! sin depender de que el código las respete (índice único parcial, unicidad del NIT, una
//! membresía vigente por empresa, FK a los catálogos).

mod common;

use common::{error_de, esquema, hex, nuevo_applicant, TestDb};
use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Statement};
use uuid::Uuid;

const TABLAS: [&str; 5] = [
    "companies",
    "memberships",
    "company_recruiters",
    "company_invitations",
    "access_audit",
];

async fn existe(db: &TestDb, t: &str) -> i64 {
    db.count(&format!(
        "SELECT count(*) FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = '{t}'"
    ))
    .await
}

async fn columna(db: &TestDb, tabla: &str, col: &str) -> Option<(String, String)> {
    let r = db
        .conn
        .query_one(Statement::from_string(
            db.conn.get_database_backend(),
            format!(
                "SELECT data_type, is_nullable FROM information_schema.columns \
                 WHERE table_schema = 'public' AND table_name = '{tabla}' AND column_name = '{col}'"
            ),
        ))
        .await
        .unwrap()?;
    Some((
        r.try_get_by_index::<String>(0).unwrap(),
        r.try_get_by_index::<String>(1).unwrap(),
    ))
}

async fn tipo(db: &TestDb, tabla: &str, col: &str) -> String {
    columna(db, tabla, col)
        .await
        .unwrap_or_else(|| panic!("falta {tabla}.{col}"))
        .0
}

/// Inserta una empresa mínima con los FK reales. Devuelve su id.
async fn empresa(db: &TestDb, nit_hash: &str) -> Uuid {
    let id = Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO companies (id, name, tax_id, tax_id_hash, country_id, employment_sector_id) \
         VALUES ('{id}', 'Café Nuevo', '\\x01'::bytea, '\\x{nit_hash}'::bytea, \
                 (SELECT id FROM countries LIMIT 1), (SELECT id FROM employment_sectors LIMIT 1))"
    ))
    .await
    .unwrap();
    id
}

/// EM10: las tablas migran, `down` las quita sin tocar lo anterior y `up` otra vez deja el
/// mismo esquema.
#[tokio::test]
async fn em10_down_y_up_dejan_el_mismo_esquema() {
    let db = TestDb::migrated().await;
    for t in TABLAS {
        assert_eq!(existe(&db, t).await, 1, "falta {t}");
    }
    let antes = esquema(&db).await;
    Migrator::down(&db.conn, Some(1)).await.expect("down");
    for t in TABLAS {
        assert_eq!(existe(&db, t).await, 0, "down debe quitar {t}");
    }
    for t in ["users", "roles", "countries", "membership_types", "plans"] {
        assert_eq!(existe(&db, t).await, 1, "down no debe tocar {t}");
    }
    let restos = esquema(&db)
        .await
        .into_iter()
        .filter(|l| TABLAS.iter().any(|t| l.contains(&format!("{t}."))))
        .count();
    assert_eq!(restos, 0);
    Migrator::up(&db.conn, None).await.expect("up");
    assert_eq!(esquema(&db).await, antes);
    db.cleanup().await;
}

/// EM10: `verified_at` es nulo por defecto y la empresa nace sin verificar.
#[tokio::test]
async fn em10_verified_at_es_nulo_por_defecto() {
    let db = TestDb::migrated().await;
    assert_eq!(
        columna(&db, "companies", "verified_at").await,
        Some(("timestamp with time zone".into(), "YES".into()))
    );
    let id = empresa(&db, "aa").await;
    assert_eq!(
        db.count(&format!(
            "SELECT count(*) FROM companies WHERE id = '{id}' AND verified_at IS NULL"
        ))
        .await,
        1
    );
    db.cleanup().await;
}

/// El NIT va cifrado (bytea) más su HMAC (bytea) para la unicidad; nada en claro.
#[tokio::test]
async fn el_nit_es_bytea_y_su_hash_es_unico() {
    let db = TestDb::migrated().await;
    assert_eq!(tipo(&db, "companies", "tax_id").await, "bytea");
    assert_eq!(tipo(&db, "companies", "tax_id_hash").await, "bytea");
    empresa(&db, "bb").await;
    let e = error_de(
        db.exec(&format!(
            "INSERT INTO companies (id, name, tax_id, tax_id_hash, country_id, employment_sector_id) \
             VALUES ('{}', 'Otra', '\\x02'::bytea, '\\xbb'::bytea, \
             (SELECT id FROM countries LIMIT 1), (SELECT id FROM employment_sectors LIMIT 1))",
            Uuid::now_v7()
        ))
        .await,
    );
    assert!(e.contains("tax_id_hash"), "{e}");
    db.cleanup().await;
}

/// Las FK de la empresa a los catálogos existen y se hacen cumplir.
#[tokio::test]
async fn la_empresa_tiene_fk_a_pais_y_sector() {
    let db = TestDb::migrated().await;
    for (col, destino) in [
        ("country_id", "countries"),
        ("employment_sector_id", "employment_sectors"),
    ] {
        let n = db
            .count(&format!(
                "SELECT count(*) FROM pg_constraint WHERE contype = 'f' \
                 AND conrelid = 'companies'::regclass AND confrelid = '{destino}'::regclass \
                 AND pg_get_constraintdef(oid) LIKE 'FOREIGN KEY ({col}) REFERENCES {destino}(id)%'"
            ))
            .await;
        assert_eq!(n, 1, "companies.{col} sin FK a {destino}");
    }
    let e = error_de(
        db.exec(&format!(
            "INSERT INTO companies (id, name, tax_id, tax_id_hash, country_id, employment_sector_id) \
             VALUES ('{}', 'X', '\\x01'::bytea, '\\xcc'::bytea, '{}', \
             (SELECT id FROM employment_sectors LIMIT 1))",
            Uuid::now_v7(),
            Uuid::now_v7()
        ))
        .await,
    );
    assert!(e.contains("foreign key") || e.contains("violates"), "{e}");
    db.cleanup().await;
}

/// Una persona, una empresa: el índice único parcial lo garantiza la base, y quien salió
/// (`deleted_at`) puede entrar a otra.
#[tokio::test]
async fn una_persona_una_empresa_por_indice_unico_parcial() {
    let db = TestDb::migrated().await;
    let (user, _) = nuevo_applicant(&db, "a").await;
    let a = empresa(&db, "01").await;
    let b = empresa(&db, "02").await;
    let alta = |emp: Uuid, extra: &str| {
        format!(
            "INSERT INTO company_recruiters (id, user_id, company_id, role{}) \
             VALUES ('{}', '{user}', '{emp}', 'company_admin'{})",
            if extra.is_empty() { "" } else { ", deleted_at" },
            Uuid::now_v7(),
            if extra.is_empty() {
                String::new()
            } else {
                format!(", {extra}")
            }
        )
    };
    db.exec(&alta(a, "")).await.unwrap();
    let e = error_de(db.exec(&alta(b, "")).await);
    assert!(e.contains("duplicate") || e.contains("unique"), "{e}");
    // La fila borrada no cuenta; y una fila borrada tampoco estorba a la nueva.
    db.exec(&format!(
        "UPDATE company_recruiters SET deleted_at = now() WHERE user_id = '{user}'"
    ))
    .await
    .unwrap();
    db.exec(&alta(b, "")).await.unwrap();
    // El índice es parcial: se ve en su definición.
    let def = db
        .count(
            "SELECT count(*) FROM pg_indexes WHERE tablename = 'company_recruiters' \
             AND indexdef LIKE '%UNIQUE%(user_id)%WHERE%deleted_at IS NULL%'",
        )
        .await;
    assert_eq!(def, 1);
    db.cleanup().await;
}

/// El rol de la membresía de empresa solo puede ser uno de los dos de empresa.
#[tokio::test]
async fn el_rol_de_empresa_es_cerrado() {
    let db = TestDb::migrated().await;
    let (user, _) = nuevo_applicant(&db, "r").await;
    let emp = empresa(&db, "03").await;
    let e = error_de(
        db.exec(&format!(
            "INSERT INTO company_recruiters (id, user_id, company_id, role) \
             VALUES ('{}', '{user}', '{emp}', 'admin')",
            Uuid::now_v7()
        ))
        .await,
    );
    assert!(e.contains("check") || e.contains("violates"), "{e}");
    db.cleanup().await;
}

/// Borrar la cuenta se lleva su fila de reclutador (cascada), sin tocar la empresa.
#[tokio::test]
async fn borrar_al_usuario_borra_su_fila_de_reclutador_y_no_la_empresa() {
    let db = TestDb::migrated().await;
    let (user, _) = nuevo_applicant(&db, "c").await;
    let emp = empresa(&db, "04").await;
    db.exec(&format!(
        "INSERT INTO company_recruiters (id, user_id, company_id, role) \
         VALUES ('{}', '{user}', '{emp}', 'company_admin')",
        Uuid::now_v7()
    ))
    .await
    .unwrap();
    db.exec(&format!(
        "UPDATE companies SET created_by = '{user}' WHERE id = '{emp}'"
    ))
    .await
    .unwrap();
    db.exec(&format!("DELETE FROM users WHERE id = '{user}'"))
        .await
        .unwrap();
    assert_eq!(db.count("SELECT count(*) FROM company_recruiters").await, 0);
    assert_eq!(db.count("SELECT count(*) FROM companies").await, 1);
    assert_eq!(
        db.count("SELECT count(*) FROM companies WHERE created_by IS NULL")
            .await,
        1
    );
    db.cleanup().await;
}

/// `memberships`: fechas con los nombres de db.md, vencimiento no anterior al inicio y una sola
/// vigente (`status = 'active'`) por empresa; el historial no tiene tope.
#[tokio::test]
async fn memberships_fechas_y_una_vigente_por_empresa() {
    let db = TestDb::migrated().await;
    let emp = empresa(&db, "05").await;
    for c in ["start_date", "expiration_date"] {
        assert_eq!(tipo(&db, "memberships", c).await, "date");
    }
    let alta = |ini: &str, fin: &str, st: &str| {
        format!(
            "INSERT INTO memberships (id, company_id, membership_type_id, start_date, \
             expiration_date, status) VALUES ('{}', '{emp}', \
             (SELECT id FROM membership_types WHERE code = 'business'), '{ini}', '{fin}', '{st}')",
            Uuid::now_v7()
        )
    };
    db.exec(&alta("2026-01-01", "2026-12-31", "active"))
        .await
        .unwrap();
    let e = error_de(db.exec(&alta("2027-01-01", "2027-12-31", "active")).await);
    assert!(e.contains("duplicate") || e.contains("unique"), "{e}");
    db.exec(&alta("2025-01-01", "2025-12-31", "cancelled"))
        .await
        .unwrap();
    db.exec(&alta("2024-01-01", "2024-12-31", "cancelled"))
        .await
        .unwrap();
    db.exec(&alta("2023-01-01", "2023-12-31", "expired"))
        .await
        .unwrap();
    let e = error_de(
        db.exec(&alta("2028-02-01", "2028-01-01", "cancelled"))
            .await,
    );
    assert!(e.contains("check") || e.contains("violates"), "{e}");
    let e = error_de(db.exec(&alta("2028-01-01", "2028-02-01", "otro")).await);
    assert!(e.contains("invalid input value for enum"), "{e}");
    db.cleanup().await;
}

/// `company_invitations`: lo que el contrato exige, y el token nunca en claro (solo hash).
#[tokio::test]
async fn company_invitations_guarda_hashes_y_no_el_token() {
    let db = TestDb::migrated().await;
    for (c, t) in [
        ("token_hash", "bytea"),
        ("email_hash", "bytea"),
        ("role", "character varying"),
        ("expires_at", "timestamp with time zone"),
        ("used_at", "timestamp with time zone"),
        ("revoked_at", "timestamp with time zone"),
        ("created_by", "uuid"),
    ] {
        assert_eq!(tipo(&db, "company_invitations", c).await, t, "{c}");
    }
    for prohibida in ["token", "email", "invitee_email"] {
        assert_eq!(columna(&db, "company_invitations", prohibida).await, None);
    }
    let emp = empresa(&db, "06").await;
    let alta = |hash: &str, rol: &str| {
        format!(
            "INSERT INTO company_invitations (id, company_id, email_hash, role, token_hash, \
             expires_at) VALUES ('{}', '{emp}', '\\x01'::bytea, '{rol}', '\\x{hash}'::bytea, \
             now() + interval '7 days')",
            Uuid::now_v7()
        )
    };
    db.exec(&alta("0a", "recruiter")).await.unwrap();
    let e = error_de(db.exec(&alta("0a", "recruiter")).await);
    assert!(e.contains("duplicate") || e.contains("unique"), "{e}");
    let e = error_de(db.exec(&alta("0b", "admin")).await);
    assert!(e.contains("check") || e.contains("violates"), "{e}");
    db.cleanup().await;
}

/// `access_audit`: ip hasheada y sin columnas de contenido.
#[tokio::test]
async fn access_audit_lleva_ip_hash_y_ningun_contenido() {
    let db = TestDb::migrated().await;
    assert_eq!(tipo(&db, "access_audit", "ip_hash").await, "bytea");
    for c in [
        "actor_user_id",
        "action",
        "resource_type",
        "resource_id",
        "created_at",
    ] {
        assert!(columna(&db, "access_audit", c).await.is_some(), "falta {c}");
    }
    for c in ["ip", "payload", "content", "details", "body", "data"] {
        assert_eq!(
            columna(&db, "access_audit", c).await,
            None,
            "no debe haber {c}"
        );
    }
    let (user, _) = nuevo_applicant(&db, "au").await;
    db.exec(&format!(
        "INSERT INTO access_audit (id, actor_user_id, action, resource_type, ip_hash) \
         VALUES ('{}', '{user}', 'company.list', 'company', '\\x{}'::bytea)",
        Uuid::now_v7(),
        hex(b"x")
    ))
    .await
    .unwrap();
    // La auditoría sobrevive a la cuenta, sin su id.
    db.exec(&format!("DELETE FROM users WHERE id = '{user}'"))
        .await
        .unwrap();
    assert_eq!(
        db.count("SELECT count(*) FROM access_audit WHERE actor_user_id IS NULL")
            .await,
        1
    );
    db.cleanup().await;
}

/// Esta migración no toca `job_offers` (la hace otro paso): no la crea ni la menciona.
#[tokio::test]
async fn la_migracion_no_toca_job_offers() {
    let db = TestDb::migrated().await;
    let n = db
        .count(
            "SELECT count(*) FROM pg_constraint WHERE conrelid IN \
             ('companies'::regclass, 'memberships'::regclass, 'company_recruiters'::regclass, \
              'company_invitations'::regclass, 'access_audit'::regclass) \
             AND pg_get_constraintdef(oid) LIKE '%job_offers%'",
        )
        .await;
    assert_eq!(n, 0);
    db.cleanup().await;
}

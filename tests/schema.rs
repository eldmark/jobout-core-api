//! Casos de verdad del esquema (E1–E15 de docs/working/fase1-identidad.md).
//! E7 (UUID v7) es unitaria y vive en `migration/src/lib.rs`.
//! Corren contra PostgreSQL real; ver `tests/common/mod.rs`.

mod common;

use base64::{engine::general_purpose::STANDARD, Engine};
use common::TestDb;
use jobout_core_api::crypto::PiiKeys;
use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Statement};
use uuid::Uuid;

const TABLAS: [&str; 11] = [
    "users",
    "user_credentials",
    "refresh_tokens",
    "roles",
    "permissions",
    "role_permissions",
    "user_roles",
    "applicants",
    "privacy_events",
    "plans",
    "user_subscriptions",
];

async fn esquema(db: &TestDb) -> Vec<String> {
    let rows = db
        .conn
        .query_all(Statement::from_string(
            db.conn.get_database_backend(),
            "SELECT table_name || '.' || column_name || ':' || data_type || ':' || is_nullable \
             FROM information_schema.columns WHERE table_schema = 'public' \
             UNION ALL SELECT 'idx:' || indexdef FROM pg_indexes WHERE schemaname = 'public' \
             UNION ALL SELECT 'enum:' || t.typname || '=' || e.enumlabel \
             FROM pg_type t JOIN pg_enum e ON e.enumtypid = t.oid \
             ORDER BY 1"
                .to_string(),
        ))
        .await
        .unwrap();
    rows.iter()
        .map(|r| r.try_get_by_index::<String>(0).unwrap())
        .collect()
}

async fn nuevo_usuario(db: &TestDb, hash: &str) -> Uuid {
    let id = Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO users (id, email, email_hash, first_name, last_name) \
         VALUES ('{id}', '\\x01'::bytea, '\\x{hash}'::bytea, 'Ana', 'Paz')"
    ))
    .await
    .unwrap();
    id
}

async fn plan_id(db: &TestDb, code: &str) -> Uuid {
    let row = db
        .conn
        .query_one(Statement::from_string(
            db.conn.get_database_backend(),
            format!("SELECT id FROM plans WHERE code = '{code}'"),
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get_by_index::<Uuid>(0).unwrap()
}

/// E1: migrar una base vacía crea todas las tablas.
#[tokio::test]
async fn e1_las_migraciones_crean_todas_las_tablas() {
    let db = TestDb::migrated().await;
    for t in TABLAS {
        let n = db
            .count(&format!(
                "SELECT count(*) FROM information_schema.tables \
                 WHERE table_schema = 'public' AND table_name = '{t}'"
            ))
            .await;
        assert_eq!(n, 1, "falta la tabla {t}");
    }
    db.cleanup().await;
}

/// E2: down seguido de up deja el mismo esquema, y down deja la base sin tablas.
#[tokio::test]
async fn e2_up_down_up_es_reversible() {
    let db = TestDb::migrated().await;
    let antes = esquema(&db).await;
    assert!(!antes.is_empty());

    Migrator::down(&db.conn, None).await.expect("down");
    let tablas = db
        .count(
            "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public' \
             AND table_name <> 'seaql_migrations'",
        )
        .await;
    assert_eq!(tablas, 0, "down debe quitar todas las tablas");
    let tipos = db
        .count("SELECT count(*) FROM pg_type WHERE typname IN ('user_status','privacy_event_kind','subscription_status','subscription_source')")
        .await;
    assert_eq!(tipos, 0, "down debe quitar los enums");

    Migrator::up(&db.conn, None).await.expect("up otra vez");
    assert_eq!(esquema(&db).await, antes);
    db.cleanup().await;
}

/// E3: dos users con el mismo email_hash fallan.
#[tokio::test]
async fn e3_email_hash_unico() {
    let db = TestDb::migrated().await;
    nuevo_usuario(&db, "aa").await;
    let dup = db
        .exec(&format!(
            "INSERT INTO users (id, email, email_hash, first_name, last_name) \
             VALUES ('{}', '\\x02'::bytea, '\\xaa'::bytea, 'Beto', 'Gil')",
            Uuid::now_v7()
        ))
        .await;
    assert!(dup.is_err());
    db.cleanup().await;
}

/// E4: applicants con user_id inexistente falla (FK).
#[tokio::test]
async fn e4_applicant_requiere_usuario() {
    let db = TestDb::migrated().await;
    let r = db
        .exec(&format!(
            "INSERT INTO applicants (id, user_id) VALUES ('{}', '{}')",
            Uuid::now_v7(),
            Uuid::now_v7()
        ))
        .await;
    let err = r.expect_err("debía fallar").to_string();
    assert!(
        err.contains("foreign key"),
        "se esperaba violación de FK: {err}"
    );
    db.cleanup().await;
}

/// E5: dos applicants para el mismo user_id fallan.
#[tokio::test]
async fn e5_un_applicant_por_usuario() {
    let db = TestDb::migrated().await;
    let u = nuevo_usuario(&db, "aa").await;
    let ins = |id: Uuid| format!("INSERT INTO applicants (id, user_id) VALUES ('{id}', '{u}')");
    db.exec(&ins(Uuid::now_v7())).await.unwrap();
    assert!(db.exec(&ins(Uuid::now_v7())).await.is_err());
    db.cleanup().await;
}

/// E6: status fuera del enum falla; los valores válidos pasan.
#[tokio::test]
async fn e6_status_es_enum() {
    let db = TestDb::migrated().await;
    let u = nuevo_usuario(&db, "aa").await;
    for ok in ["active", "suspended", "pending_verification"] {
        db.exec(&format!(
            "UPDATE users SET status = '{ok}' WHERE id = '{u}'"
        ))
        .await
        .unwrap();
    }
    let r = db
        .exec(&format!(
            "UPDATE users SET status = 'banned' WHERE id = '{u}'"
        ))
        .await;
    assert!(r.is_err());
    db.cleanup().await;
}

/// E8: lo guardado en users.email no contiene el email en claro y se descifra de vuelta.
#[tokio::test]
async fn e8_email_no_esta_en_claro() {
    let db = TestDb::migrated().await;
    let k = PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap();
    let email = "ana.paz@mail.com";
    let cifrado = k.encrypt(email).unwrap();
    let hash = k.email_hash(email);
    let id = Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO users (id, email, email_hash, first_name, last_name) \
         VALUES ('{id}', '\\x{}'::bytea, '\\x{}'::bytea, 'Ana', 'Paz')",
        hex(&cifrado),
        hex(&hash)
    ))
    .await
    .unwrap();

    let row = db
        .conn
        .query_one(Statement::from_string(
            db.conn.get_database_backend(),
            format!("SELECT email FROM users WHERE id = '{id}'"),
        ))
        .await
        .unwrap()
        .unwrap();
    let guardado: Vec<u8> = row.try_get_by_index(0).unwrap();
    assert!(!String::from_utf8_lossy(&guardado).contains(email));
    assert!(!guardado.windows(email.len()).any(|w| w == email.as_bytes()));
    assert_eq!(k.decrypt(&guardado).unwrap(), email);
    db.cleanup().await;
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// E9: borrar un user borra en cascada sus dependientes.
#[tokio::test]
async fn e9_borrado_en_cascada() {
    let db = TestDb::migrated().await;
    let u = nuevo_usuario(&db, "aa").await;
    let otro = nuevo_usuario(&db, "bb").await;
    let free = plan_id(&db, "free").await;
    for id in [u, otro] {
        db.exec(&format!(
            "INSERT INTO applicants (id, user_id) VALUES ('{}', '{id}')",
            Uuid::now_v7()
        ))
        .await
        .unwrap();
        db.exec(&format!(
            "INSERT INTO user_roles (user_id, role_id) \
             SELECT '{id}', id FROM roles WHERE code = 'applicant'"
        ))
        .await
        .unwrap();
        db.exec(&format!(
            "INSERT INTO user_credentials (user_id, password_hash) VALUES ('{id}', '$argon2id$x')"
        ))
        .await
        .unwrap();
        db.exec(&format!(
            "INSERT INTO refresh_tokens (id, user_id, token_hash, expires_at) \
             VALUES ('{}', '{id}', decode(replace('{id}', '-', ''), 'hex'), now() + interval '30 days')",
            Uuid::now_v7()
        ))
        .await
        .unwrap();
        db.exec(&format!(
            "INSERT INTO user_subscriptions (id, user_id, plan_id, status, source) \
             VALUES ('{}', '{id}', '{free}', 'active', 'free')",
            Uuid::now_v7()
        ))
        .await
        .unwrap();
    }

    db.exec(&format!("DELETE FROM users WHERE id = '{u}'"))
        .await
        .unwrap();

    for t in [
        "applicants",
        "user_roles",
        "user_credentials",
        "refresh_tokens",
        "user_subscriptions",
    ] {
        assert_eq!(
            db.count(&format!("SELECT count(*) FROM {t} WHERE user_id = '{u}'"))
                .await,
            0,
            "{t} debía borrarse"
        );
        assert_eq!(
            db.count(&format!(
                "SELECT count(*) FROM {t} WHERE user_id = '{otro}'"
            ))
            .await,
            1,
            "{t} del otro usuario debía quedar"
        );
    }
    db.cleanup().await;
}

/// E10: borrar un user no borra privacy_events (y no hay FK).
#[tokio::test]
async fn e10_privacy_events_sobrevive() {
    let db = TestDb::migrated().await;
    let u = nuevo_usuario(&db, "aa").await;
    db.exec(&format!(
        "INSERT INTO privacy_events (id, subject_hash, kind) \
         VALUES ('{}', '\\xaa'::bytea, 'account_deleted')",
        Uuid::now_v7()
    ))
    .await
    .unwrap();
    db.exec(&format!("DELETE FROM users WHERE id = '{u}'"))
        .await
        .unwrap();
    assert_eq!(db.count("SELECT count(*) FROM privacy_events").await, 1);
    let fks = db
        .count(
            "SELECT count(*) FROM information_schema.table_constraints \
             WHERE table_name = 'privacy_events' AND constraint_type = 'FOREIGN KEY'",
        )
        .await;
    assert_eq!(fks, 0, "privacy_events no debe tener FK");
    db.cleanup().await;
}

/// E11: se siembran los cuatro planes con su precio.
#[tokio::test]
async fn e11_planes_sembrados() {
    let db = TestDb::migrated().await;
    for (code, precio) in [
        ("free", "0.00"),
        ("student", "60.00"),
        ("standard", "75.00"),
        ("business", "487.50"),
    ] {
        let n = db
            .count(&format!(
                "SELECT count(*) FROM plans WHERE code = '{code}' \
                 AND monthly_price_gtq = {precio}"
            ))
            .await;
        assert_eq!(n, 1, "plan {code} con precio {precio}");
    }
    assert_eq!(db.count("SELECT count(*) FROM plans").await, 4);
    // el plan empresarial no tiene tope de simulaciones
    assert_eq!(
        db.count("SELECT count(*) FROM plans WHERE monthly_simulations IS NULL")
            .await,
        1
    );
    db.cleanup().await;
}

async fn suscribir(db: &TestDb, u: Uuid, plan: Uuid, status: &str) -> Result<(), sea_orm::DbErr> {
    db.exec(&format!(
        "INSERT INTO user_subscriptions (id, user_id, plan_id, status, source) \
         VALUES ('{}', '{u}', '{plan}', '{status}', 'paid')",
        Uuid::now_v7()
    ))
    .await
}

/// E12: no puede haber dos suscripciones activas del mismo usuario.
#[tokio::test]
async fn e12_una_sola_suscripcion_activa() {
    let db = TestDb::migrated().await;
    let u = nuevo_usuario(&db, "aa").await;
    let otro = nuevo_usuario(&db, "bb").await;
    let plan = plan_id(&db, "student").await;
    suscribir(&db, u, plan, "active").await.unwrap();
    assert!(suscribir(&db, u, plan, "active").await.is_err());
    // otro usuario sí puede tener la suya
    suscribir(&db, otro, plan, "active").await.unwrap();
    db.cleanup().await;
}

/// E13: una activa y varias expired/cancelled conviven.
#[tokio::test]
async fn e13_una_activa_y_varias_historicas() {
    let db = TestDb::migrated().await;
    let u = nuevo_usuario(&db, "aa").await;
    let plan = plan_id(&db, "standard").await;
    suscribir(&db, u, plan, "active").await.unwrap();
    suscribir(&db, u, plan, "expired").await.unwrap();
    suscribir(&db, u, plan, "expired").await.unwrap();
    suscribir(&db, u, plan, "cancelled").await.unwrap();
    assert_eq!(
        db.count(&format!(
            "SELECT count(*) FROM user_subscriptions WHERE user_id = '{u}'"
        ))
        .await,
        4
    );
    // el enum rechaza estados y orígenes inventados
    assert!(suscribir(&db, u, plan, "paused").await.is_err());
    let r = db
        .exec(&format!(
            "INSERT INTO user_subscriptions (id, user_id, plan_id, status, source) \
             VALUES ('{}', '{u}', '{plan}', 'expired', 'regalo')",
            Uuid::now_v7()
        ))
        .await;
    assert!(r.is_err());
    db.cleanup().await;
}

/// E14: plans.code es único.
#[tokio::test]
async fn e14_codigo_de_plan_unico() {
    let db = TestDb::migrated().await;
    let r = db
        .exec(&format!(
            "INSERT INTO plans (id, code, name, monthly_price_gtq) \
             VALUES ('{}', 'free', 'Otro', 1)",
            Uuid::now_v7()
        ))
        .await;
    let err = r.expect_err("debía fallar").to_string();
    assert!(
        err.contains("unique"),
        "se esperaba violación de unicidad: {err}"
    );
    db.cleanup().await;
}

/// E15: se siembran los roles applicant y admin.
#[tokio::test]
async fn e15_roles_sembrados() {
    let db = TestDb::migrated().await;
    assert_eq!(
        db.count("SELECT count(*) FROM roles WHERE code IN ('applicant','admin')")
            .await,
        2
    );
    db.cleanup().await;
}

/// Complemento de E1/clerk: clerk_user_id acepta NULL (varios usuarios sin Clerk) y es único si existe.
#[tokio::test]
async fn clerk_user_id_es_nullable_y_unico() {
    let db = TestDb::migrated().await;
    nuevo_usuario(&db, "aa").await;
    nuevo_usuario(&db, "bb").await; // dos NULL conviven
    let ins = |hash: &str| {
        format!(
            "INSERT INTO users (id, clerk_user_id, email, email_hash, first_name, last_name) \
             VALUES ('{}', 'user_1', '\\x01'::bytea, '\\x{hash}'::bytea, 'A', 'B')",
            Uuid::now_v7()
        )
    };
    db.exec(&ins("cc")).await.unwrap();
    assert!(db.exec(&ins("dd")).await.is_err());
    db.cleanup().await;
}

//! Esquema de `usage_ledger` (migración `m20261108_000003`): reversible, con las FK en
//! `SET NULL` (la fila sobrevive a la cuenta y a la sesión) y sin valores negativos.

mod common;

use common::{error_de, esquema, nuevo_applicant, TestDb};
use migration::{Migrator, MigratorTrait};
use uuid::Uuid;

async fn existe(db: &TestDb) -> bool {
    db.count(
        "SELECT count(*) FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = 'usage_ledger'",
    )
    .await
        == 1
}

async fn fila(db: &TestDb, user: Option<Uuid>, units: i32, cost: i64) -> Result<(), String> {
    let user = user.map_or("NULL".to_string(), |u| format!("'{u}'"));
    db.exec(&format!(
        "INSERT INTO usage_ledger (id, user_id, provider, operation, units, cost_micros) \
         VALUES ('{}', {user}, 'deterministic', 'interview_session', {units}, {cost})",
        Uuid::now_v7()
    ))
    .await
    .map_err(|e| e.to_string())
}

#[tokio::test]
async fn u1_la_tabla_es_reversible() {
    let db = TestDb::migrated().await;
    assert!(existe(&db).await);
    let antes = esquema(&db).await;
    Migrator::down(&db.conn, Some(3)).await.expect("down");
    assert!(!existe(&db).await, "down debe quitar usage_ledger");
    assert!(
        db.count("SELECT count(*) FROM information_schema.tables WHERE table_name = 'interview_sessions'")
            .await
            == 1,
        "down solo quita lo suyo"
    );
    Migrator::up(&db.conn, None).await.expect("up");
    assert_eq!(esquema(&db).await, antes);
    db.cleanup().await;
}

#[tokio::test]
async fn u2_borrar_al_usuario_deja_la_fila_sin_su_id() {
    let db = TestDb::migrated().await;
    let (user, _) = nuevo_applicant(&db, "u2").await;
    fila(&db, Some(user), 1, 250).await.unwrap();
    db.exec(&format!("DELETE FROM users WHERE id = '{user}'"))
        .await
        .unwrap();
    assert_eq!(
        db.count("SELECT count(*) FROM usage_ledger WHERE user_id IS NULL AND cost_micros = 250")
            .await,
        1
    );
    db.cleanup().await;
}

#[tokio::test]
async fn u3_no_admite_unidades_ni_costo_negativos_ni_proveedor_nulo() {
    let db = TestDb::migrated().await;
    assert!(
        error_de(fila(&db, None, -1, 0).await.map_err(sea_orm::DbErr::Custom)).contains("units")
    );
    assert!(
        error_de(fila(&db, None, 1, -5).await.map_err(sea_orm::DbErr::Custom))
            .contains("cost_micros")
    );
    let r = db
        .exec(&format!(
            "INSERT INTO usage_ledger (id, provider, operation, units) VALUES ('{}', NULL, 'x', 1)",
            Uuid::now_v7()
        ))
        .await;
    assert!(error_de(r).contains("provider"));
    // El costo por defecto es 0.
    db.exec(&format!(
        "INSERT INTO usage_ledger (id, provider, operation, units) VALUES ('{}', 'p', 'o', 1)",
        Uuid::now_v7()
    ))
    .await
    .unwrap();
    assert_eq!(
        db.count("SELECT count(*) FROM usage_ledger WHERE cost_micros = 0")
            .await,
        1
    );
    db.cleanup().await;
}

//! Casos de verdad del contador de intentos fallidos (módulo `login_attempts`).
//!
//! Antes el SQL estaba copiado en `auth.rs` y `me.rs`; ahora vive en un solo módulo y se prueba
//! directamente contra PostgreSQL real. Se escribieron antes que el módulo, en rojo.

mod common;

use common::TestDb;
use jobout_core_api::login_attempts::{self, MAX_FAILURES, WINDOW_MINUTES};

fn hash(n: u8) -> Vec<u8> {
    vec![n; 32]
}

/// L7: los valores acordados en el plan (5 fallos en 15 minutos).
#[test]
fn l7_los_limites_son_los_acordados() {
    assert_eq!(MAX_FAILURES, 5);
    assert_eq!(WINDOW_MINUTES, 15);
}

/// L1: sin historial no está bloqueado.
#[tokio::test]
async fn l1_sin_intentos_no_esta_bloqueado() {
    let db = TestDb::migrated().await;
    assert!(!login_attempts::is_locked(&db.conn, &hash(1)).await.unwrap());
    db.cleanup().await;
}

/// L2: se bloquea exactamente en el quinto fallo.
#[tokio::test]
async fn l2_el_quinto_fallo_bloquea() {
    let db = TestDb::migrated().await;
    for i in 1..=MAX_FAILURES {
        assert!(
            !login_attempts::is_locked(&db.conn, &hash(1)).await.unwrap(),
            "bloqueado antes de tiempo con {} fallos",
            i - 1
        );
        login_attempts::record_failure(&db.conn, &hash(1))
            .await
            .unwrap();
    }
    assert!(login_attempts::is_locked(&db.conn, &hash(1)).await.unwrap());
    db.cleanup().await;
}

/// L3: cada email_hash cuenta aparte.
#[tokio::test]
async fn l3_los_contadores_son_independientes() {
    let db = TestDb::migrated().await;
    for _ in 0..MAX_FAILURES {
        login_attempts::record_failure(&db.conn, &hash(1))
            .await
            .unwrap();
    }
    assert!(login_attempts::is_locked(&db.conn, &hash(1)).await.unwrap());
    assert!(!login_attempts::is_locked(&db.conn, &hash(2)).await.unwrap());
    db.cleanup().await;
}

/// L5: dentro de la ventana los fallos se acumulan.
#[tokio::test]
async fn l5_dentro_de_la_ventana_los_fallos_se_acumulan() {
    let db = TestDb::migrated().await;
    for _ in 0..3 {
        login_attempts::record_failure(&db.conn, &hash(1))
            .await
            .unwrap();
    }
    assert_eq!(
        db.count("SELECT failures::bigint FROM login_attempts")
            .await,
        3
    );
    db.cleanup().await;
}

/// L4: pasada la ventana el bloqueo vence, y el siguiente fallo reinicia la cuenta en 1.
#[tokio::test]
async fn l4_la_ventana_vencida_libera_y_reinicia() {
    let db = TestDb::migrated().await;
    for _ in 0..MAX_FAILURES {
        login_attempts::record_failure(&db.conn, &hash(1))
            .await
            .unwrap();
    }
    db.exec("UPDATE login_attempts SET window_started_at = now() - interval '16 minutes'")
        .await
        .unwrap();
    assert!(!login_attempts::is_locked(&db.conn, &hash(1)).await.unwrap());

    login_attempts::record_failure(&db.conn, &hash(1))
        .await
        .unwrap();
    assert_eq!(
        db.count("SELECT failures::bigint FROM login_attempts")
            .await,
        1
    );
    assert_eq!(
        db.count("SELECT count(*) FROM login_attempts WHERE window_started_at > now() - interval '1 minute'")
            .await,
        1,
        "la ventana no se reinició"
    );
    db.cleanup().await;
}

/// L6: `clear` borra el contador, y no falla si no había nada.
#[tokio::test]
async fn l6_clear_borra_y_es_idempotente() {
    let db = TestDb::migrated().await;
    for _ in 0..MAX_FAILURES {
        login_attempts::record_failure(&db.conn, &hash(1))
            .await
            .unwrap();
    }
    login_attempts::clear(&db.conn, &hash(1)).await.unwrap();
    assert!(!login_attempts::is_locked(&db.conn, &hash(1)).await.unwrap());
    assert_eq!(db.count("SELECT count(*) FROM login_attempts").await, 0);
    login_attempts::clear(&db.conn, &hash(1)).await.unwrap();
    db.cleanup().await;
}

/// L8: cinco fallos simultáneos no pierden ninguno (la operación es atómica).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn l8_fallos_simultaneos_no_se_pierden() {
    let db = TestDb::migrated().await;
    let tareas: Vec<_> = (0..MAX_FAILURES)
        .map(|_| {
            let conn = db.conn.clone();
            tokio::spawn(async move { login_attempts::record_failure(&conn, &hash(1)).await })
        })
        .collect();
    for t in tareas {
        t.await.unwrap().unwrap();
    }
    assert_eq!(
        db.count("SELECT failures::bigint FROM login_attempts")
            .await,
        MAX_FAILURES as i64
    );
    assert!(login_attempts::is_locked(&db.conn, &hash(1)).await.unwrap());
    db.cleanup().await;
}

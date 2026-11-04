//! S5: toda ruta exige `AuthUser`, salvo una lista explícita y justificada.
//!
//! Axum no permite listar las rutas de un `Router`, así que el registro vive en
//! `routes::registry` y estas pruebas lo cruzan con dos fuentes independientes:
//!   1. el código fuente (`.route("...")` en `src/`): una ruta montada sin declarar falla;
//!   2. el comportamiento: cada ruta protegida declarada responde 401 sin token.

#[allow(dead_code)]
mod common;

use std::{collections::BTreeSet, fs, path::Path, sync::Arc};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use common::TestDb;
use jobout_core_api::{
    app_with_state, crypto::PiiKeys, jwt::JwtKeys, routes::registry, state::AppState,
};
use tower::ServiceExt;

/// Rutas literales `.route("...")` encontradas en `src/`, recorriendo todos los `.rs`.
fn rutas_en_el_codigo() -> BTreeSet<String> {
    fn recorrer(dir: &Path, rutas: &mut BTreeSet<String>) {
        for entrada in fs::read_dir(dir).unwrap() {
            let ruta = entrada.unwrap().path();
            if ruta.is_dir() {
                recorrer(&ruta, rutas);
            } else if ruta.extension().is_some_and(|e| e == "rs") {
                let texto = fs::read_to_string(&ruta).unwrap();
                for trozo in texto.split(".route(").skip(1) {
                    let trozo = trozo.trim_start();
                    if let Some(resto) = trozo.strip_prefix('"') {
                        rutas.insert(resto.split('"').next().unwrap().to_string());
                    } else if !trozo.starts_with(')') {
                        panic!(
                            "{}: `.route(` sin un literal de ruta; el registro S5 no puede \
                             verla. Usa un literal.",
                            ruta.display()
                        );
                    }
                }
            }
        }
    }
    let mut rutas = BTreeSet::new();
    recorrer(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut rutas,
    );
    rutas
}

/// Toda ruta montada está declarada como pública (con justificación) o protegida.
#[test]
fn s5_toda_ruta_montada_esta_declarada() {
    let declaradas: BTreeSet<String> = registry::PUBLIC
        .iter()
        .map(|r| r.path.to_string())
        .chain(registry::PROTECTED.iter().map(|r| r.path.to_string()))
        .collect();
    let sin_declarar: Vec<_> = rutas_en_el_codigo()
        .difference(&declaradas)
        .cloned()
        .collect();
    assert!(
        sin_declarar.is_empty(),
        "rutas montadas que no están en routes::registry (PUBLIC o PROTECTED): {sin_declarar:?}"
    );
}

/// Las protegidas declaradas existen en el código, y las públicas llevan justificación.
#[test]
fn s5_el_registro_es_coherente() {
    let en_codigo = rutas_en_el_codigo();
    for ruta in registry::PROTECTED {
        assert!(
            en_codigo.contains(ruta.path),
            "{} no está montada",
            ruta.path
        );
    }
    for ruta in registry::PUBLIC {
        assert!(
            ruta.reason.trim().len() >= 10,
            "{} es pública sin una justificación real",
            ruta.path
        );
        assert!(
            !registry::PROTECTED.iter().any(|p| p.path == ruta.path),
            "{} está en ambas listas",
            ruta.path
        );
    }
    assert!(!registry::PROTECTED.is_empty());
}

/// Cada ruta protegida declarada responde 401 sin token, para cada método declarado.
#[tokio::test]
async fn s5_las_rutas_protegidas_dan_401_sin_token() {
    let db = TestDb::migrated().await;
    let app = app_with_state(AppState {
        db: db.conn.clone(),
        pii: Arc::new(
            PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
        ),
        jwt: Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap()),
    });
    for ruta in registry::PROTECTED {
        for metodo in ruta.methods {
            let respuesta = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(*metodo)
                        .uri(ruta.path)
                        .header("content-type", "application/json")
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                respuesta.status(),
                StatusCode::UNAUTHORIZED,
                "{metodo} {} no exige AuthUser",
                ruta.path
            );
        }
    }
    db.cleanup().await;
}

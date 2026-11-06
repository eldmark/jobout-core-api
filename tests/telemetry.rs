//! El filtro de logs de producción (`telemetry::env_filter`).
//!
//! Hallazgo de la Fase 2: `sea-orm` abre un *span* de nivel `trace` por cada consulta con la
//! sentencia **y sus valores** como campos, y cualquier evento emitido dentro (aunque sea un
//! INFO del propio servicio) los imprime como contexto. Con `RUST_LOG=trace` salían en claro
//! nombres de personas, descripciones, empresas y nombres de archivo. El filtro de producción
//! tiene que impedirlo **sin importar** qué valga `RUST_LOG`.
//!
//! Se escribieron antes que la implementación, en rojo. Ver CONTRIBUTING.md.

use std::sync::{Arc, Mutex};

use jobout_core_api::telemetry::env_filter;
use tracing_subscriber::layer::SubscriberExt;

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Buffer {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Ejecuta `f` con un suscriptor armado con el filtro de producción y devuelve lo que se escribió.
fn capturar(rust_log: Option<&str>, f: impl FnOnce()) -> String {
    let buffer = Buffer::default();
    let escritor = buffer.clone();
    let capa = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(move || escritor.clone());
    let suscriptor = tracing_subscriber::registry()
        .with(env_filter(rust_log))
        .with(capa);
    tracing::subscriber::with_default(suscriptor, f);
    let bytes = buffer.0.lock().unwrap().clone();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Simula lo que hace `sea-orm`: un span `trace` con la sentencia y un evento del servicio dentro.
fn consulta_con_valores() {
    let span = tracing::trace_span!(
        target: "sea_orm::database::db_connection",
        "execute",
        stmt = "INSERT INTO users VALUES ('Ana Lopez')"
    );
    let _dentro = span.enter();
    tracing::info!(target: "jobout_core_api::routes::cvs", "evento del servicio");
}

/// T1: con `RUST_LOG=trace` los valores de las consultas NO llegan al log.
#[test]
fn t1_trace_global_no_filtra_los_valores_de_las_consultas() {
    let log = capturar(Some("trace"), consulta_con_valores);
    assert!(
        log.contains("evento del servicio"),
        "el evento propio debe verse: {log}"
    );
    assert!(!log.contains("Ana Lopez"), "se filtró un valor: {log}");
    assert!(
        !log.contains("INSERT INTO"),
        "se filtró la sentencia: {log}"
    );
}

/// T2: tampoco aunque se pida `sea_orm` o `sqlx` a propósito.
#[test]
fn t2_pedir_sea_orm_a_trace_no_lo_enciende() {
    for rust_log in [
        "sea_orm=trace",
        "sqlx=trace",
        "trace,sea_orm=trace,sqlx=debug",
        "debug",
    ] {
        let log = capturar(Some(rust_log), consulta_con_valores);
        assert!(
            !log.contains("Ana Lopez"),
            "RUST_LOG={rust_log} filtró un valor: {log}"
        );
    }
}

/// T3: sin `RUST_LOG` el filtro por defecto deja ver lo del servicio (debug) pero no lo ajeno.
#[test]
fn t3_el_filtro_por_defecto_deja_ver_el_servicio() {
    let log = capturar(None, || {
        tracing::debug!(target: "jobout_core_api::x", "debug del servicio");
        tracing::debug!(target: "otra_crate::x", "debug ajeno");
    });
    assert!(log.contains("debug del servicio"));
    assert!(!log.contains("debug ajeno"));
}

/// T4: un `RUST_LOG` vacío o inválido cae al filtro por defecto, no hace `panic`.
#[test]
fn t4_rust_log_vacio_o_invalido_usa_el_defecto() {
    for rust_log in ["", "   ", "esto=no=es=un=filtro=[[["] {
        let log = capturar(Some(rust_log), || {
            tracing::debug!(target: "jobout_core_api::x", "debug del servicio");
        });
        assert!(
            log.contains("debug del servicio"),
            "RUST_LOG={rust_log:?}: {log}"
        );
    }
}

/// T5: los errores y avisos de `sea_orm` siguen viéndose (solo se corta el detalle).
#[test]
fn t5_los_avisos_de_sea_orm_siguen_visibles() {
    let log = capturar(Some("trace"), || {
        tracing::warn!(target: "sea_orm::driver", "conexión lenta");
    });
    assert!(log.contains("conexión lenta"), "{log}");
}

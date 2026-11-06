//! Filtro de logs de producción.
//!
//! `sea-orm` abre un *span* de nivel `trace` por cada consulta con la sentencia **y sus valores**
//! como campos, y cualquier evento emitido dentro de ese span (incluso un INFO del propio
//! servicio) los imprime como contexto. Varios valores van en claro (nombres, descripción del
//! perfil, empresa, cargo, nombre del archivo), así que un `RUST_LOG=trace` puesto «solo un
//! momento» en producción los llevaría a los logs.
//!
//! Por eso este filtro **fija por código** que `sea_orm`, `sqlx` y `sea_orm_migration` no
//! pasen de `warn`, sea cual sea el valor de `RUST_LOG`. Los avisos y errores de esas librerías
//! siguen viéndose; solo se corta el detalle.

use tracing_subscriber::EnvFilter;

/// Lo que se ve si `RUST_LOG` no existe o no se entiende.
pub const DEFAULT_FILTER: &str = "jobout_core_api=debug,tower_http=debug";

/// Librerías cuyo detalle nunca debe llegar al log.
const SILENCED: [&str; 3] = ["sea_orm=warn", "sqlx=warn", "sea_orm_migration=warn"];

/// Construye el filtro a partir del valor de `RUST_LOG` (si lo hay).
pub fn env_filter(rust_log: Option<&str>) -> EnvFilter {
    let pedido = rust_log.map(str::trim).filter(|s| !s.is_empty());
    let mut filtro = pedido
        .and_then(|s| EnvFilter::try_new(s).ok())
        .unwrap_or_else(|| EnvFilter::new(DEFAULT_FILTER));
    for directiva in SILENCED {
        filtro = filtro.add_directive(directiva.parse().expect("directiva constante válida"));
    }
    filtro
}

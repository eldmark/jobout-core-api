//! La tabla de rutas: la **única** forma de montar una ruta en la aplicación (S5).
//!
//! Cada ruta se declara aquí con su acceso:
//!
//! - [`Access::Public`] exige escribir **por qué** es pública. Una ruta pública sin justificación
//!   no compila, y una justificación de relleno la rechaza un test.
//! - [`Access::Authenticated`] aplica la autenticación **al montar la ruta**, con un
//!   *middleware* que valida el token y la cuenta. Aunque el handler se olvide de pedir
//!   `AuthUser`, la ruta no queda abierta.
//!
//! Los módulos de rutas no construyen `Router`s: devuelven `Vec<RouteSpec>`. Para añadir una ruta
//! se declara en el módulo que corresponda; para que sea visible en la revisión, además hay que
//! actualizar la lista esperada de `tests/route_table.rs`.
//!
//! Antes esto era una lista escrita a mano más un escaneo del código fuente. Ahora no hay
//! nada que escanear: las pruebas ejercitan la aplicación real, ruta por ruta y método por método.

use axum::{
    extract::{DefaultBodyLimit, FromRequestParts, Request, State},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::MethodRouter,
    Router,
};

use super::{auth, catalogs, health, me};
use crate::{auth_user::AuthUser, state::AppState};

/// Tope del cuerpo de cualquier petición. Todas las rutas actuales son JSON pequeños.
pub const MAX_BODY_BYTES: usize = 8 * 1024;

/// Quién puede llamar a una ruta.
#[derive(Debug, Clone, Copy)]
pub enum Access {
    /// Sin token, a propósito. El texto explica por qué.
    Public(&'static str),
    /// Exige un token válido de una cuenta vigente y no suspendida.
    Authenticated,
}

/// Una ruta de la aplicación.
pub struct RouteSpec {
    pub path: &'static str,
    /// Métodos que atiende `handler`. Un test comprueba que coinciden con los reales.
    pub methods: &'static [&'static str],
    pub access: Access,
    pub handler: MethodRouter<AppState>,
}

/// Todas las rutas de la aplicación.
pub fn all() -> Vec<RouteSpec> {
    let mut rutas = health::routes();
    rutas.extend(auth::routes());
    rutas.extend(me::routes());
    rutas.extend(catalogs::routes());
    rutas
}

/// Monta las rutas aplicando el acceso declarado. Es el único lugar donde se llama a
/// `Router::route`.
pub fn build(specs: Vec<RouteSpec>, state: AppState) -> Router {
    let mut router = Router::new();
    for spec in specs {
        let handler = match spec.access {
            Access::Public(_) => spec.handler,
            Access::Authenticated => spec
                .handler
                .route_layer(middleware::from_fn_with_state(state.clone(), require_auth)),
        };
        router = router.route(spec.path, handler);
    }
    router
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

/// Valida el token y la cuenta antes de que la petición llegue al handler, y deja el usuario
/// en las extensiones para que `AuthUser` no repita la consulta.
async fn require_auth(
    State(state): State<AppState>,
    peticion: Request,
    siguiente: Next,
) -> Response {
    let (mut partes, cuerpo) = peticion.into_parts();
    match AuthUser::from_request_parts(&mut partes, &state).await {
        Ok(usuario) => {
            partes.extensions.insert(usuario);
            siguiente.run(Request::from_parts(partes, cuerpo)).await
        }
        Err(e) => e.into_response(),
    }
}

//! La tabla de rutas: la **única** forma de montar una ruta en la aplicación (S5).
//!
//! Cada ruta se declara aquí con su acceso:
//!
//! - [`Access::Public`] exige escribir **por qué** es pública. Una ruta pública sin justificación
//!   no compila, y una justificación de relleno la rechaza un test.
//! - [`Access::Permission`] es `Authenticated` **más** un permiso concedido por los roles de la
//!   cuenta (`user_roles` → `role_permissions`). Sin él, 403 `sin_permiso`; el error no dice
//!   qué permiso faltaba. Los permisos viajan en la misma consulta de la cuenta que ya hace la
//!   autenticación, así que quitar un rol corta el acceso en la siguiente petición.
//! - [`Access::Authenticated`] aplica la autenticación **al montar la ruta**, con un
//!   *middleware* que valida el token y la cuenta. Aunque el handler se olvide de pedir
//!   `AuthUser`, la ruta no queda abierta.
//!
//! Una misma ruta puede declararse varias veces **con métodos distintos** cuando cada método
//! exige un acceso distinto (p. ej. `POST /company/offers` pide `offer.create` y
//! `GET /company/offers` pide `offer.update`): axum las une. Declarar el mismo método dos veces
//! sigue siendo un error (lo vigila `tests/route_table.rs`).
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

use super::{
    admin_companies, auth, catalogs, companies, cvs, health, interview_catalog, interviews, me,
    offers, offers_public, profile,
};
use crate::{
    auth_user::{AuthError, AuthUser},
    state::AppState,
};

/// Tope del cuerpo de cualquier petición. Todas las rutas actuales son JSON pequeños.
pub const MAX_BODY_BYTES: usize = 8 * 1024;

/// Quién puede llamar a una ruta.
#[derive(Debug, Clone, Copy)]
pub enum Access {
    /// Sin token, a propósito. El texto explica por qué.
    Public(&'static str),
    /// Exige un token válido de una cuenta vigente y no suspendida.
    Authenticated,
    /// Autenticada **y** con este permiso (`user_roles` → `role_permissions`).
    Permission(&'static str),
}

impl Access {
    /// El permiso que exige la ruta, si declara uno.
    pub fn required_permission(&self) -> Option<&'static str> {
        match self {
            Self::Permission(p) => Some(p),
            _ => None,
        }
    }
}

/// Una ruta de la aplicación.
pub struct RouteSpec {
    pub path: &'static str,
    /// Métodos que atiende `handler`. Un test comprueba que coinciden con los reales.
    pub methods: &'static [&'static str],
    pub access: Access,
    /// Tope de cuerpo propio de esta ruta, en bytes. `None` = el global (`MAX_BODY_BYTES`).
    /// Existe para la subida de archivos; nadie debería declarar más de 6 MB (lo vigila un test).
    pub body_limit: Option<usize>,
    pub handler: MethodRouter<AppState>,
}

/// Todas las rutas de la aplicación.
pub fn all() -> Vec<RouteSpec> {
    let mut rutas = health::routes();
    rutas.extend(auth::routes());
    rutas.extend(me::routes());
    rutas.extend(catalogs::routes());
    rutas.extend(cvs::routes());
    rutas.extend(profile::routes());
    rutas.extend(interviews::routes());
    rutas.extend(interview_catalog::routes());
    rutas.extend(companies::routes());
    rutas.extend(admin_companies::routes());
    rutas.extend(offers::routes());
    rutas.extend(offers_public::routes());
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
            Access::Permission(permiso) => spec.handler.route_layer(
                middleware::from_fn_with_state(state.clone(), move |s, p, n| {
                    require_permission(permiso, s, p, n)
                }),
            ),
        };
        // El tope propio va en la ruta y gana al global, que se aplica por fuera.
        let handler = match spec.body_limit {
            Some(max) => handler.layer(DefaultBodyLimit::max(max)),
            None => handler,
        };
        router = router.route(spec.path, handler);
    }
    router
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

/// Como [`require_auth`], y además exige `permiso` entre los de la cuenta.
async fn require_permission(
    permiso: &'static str,
    State(state): State<AppState>,
    peticion: Request,
    siguiente: Next,
) -> Response {
    let (mut partes, cuerpo) = peticion.into_parts();
    match AuthUser::from_request_parts(&mut partes, &state).await {
        Ok(usuario) if usuario.has_permission(permiso) => {
            partes.extensions.insert(usuario);
            siguiente.run(Request::from_parts(partes, cuerpo)).await
        }
        Ok(_) => AuthError::MissingPermission.into_response(),
        Err(e) => e.into_response(),
    }
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

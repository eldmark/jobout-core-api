//! Estado compartido de la aplicación: lo que cada ruta necesita y nada más.

use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::{crypto::PiiKeys, jwt::JwtKeys};

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub pii: Arc<PiiKeys>,
    pub jwt: Arc<JwtKeys>,
}

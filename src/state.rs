//! Estado compartido de la aplicación: lo que cada ruta necesita y nada más.

use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::{blob::BlobStore, crypto::PiiKeys, jwt::JwtKeys};

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub pii: Arc<PiiKeys>,
    pub jwt: Arc<JwtKeys>,
    /// Dónde se guardan los archivos (los CVs). Disco privado en desarrollo; S3 más adelante.
    pub blobs: Arc<dyn BlobStore>,
    /// El motor de la entrevista. Detrás del trait para que las pruebas enchufen un guion o uno que falla.
    pub engine: crate::interview::SharedEngine,
}

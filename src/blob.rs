//! Almacén de archivos (CV y futuros adjuntos).
//!
//! - El contrato es el trait [`BlobStore`], usable como `Arc<dyn BlobStore>`: guardar, leer y
//!   borrar bytes por clave. No hay "actualizar": una clave se escribe una sola vez.
//! - Las claves son opacas y aleatorias ([`new_blob_key`]); nunca llevan el nombre del archivo
//!   ni el id de la persona. Solo se aceptan `[a-z0-9-]` de 1 a 64 caracteres, así que una clave
//!   jamás puede salirse del directorio base.
//! - [`LocalBlobStore`] guarda en disco privado (directorio `0700`, archivos `0600`).
//!   [`MemoryBlobStore`] cumple el mismo contrato para las pruebas.
//!
//! Ningún error de este módulo contiene rutas del disco, claves ni contenido.

use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;

pub const BLOB_DIR_VAR: &str = "BLOB_DIR";

const MAX_KEY_LEN: usize = 64;

/// Error de una operación del almacén.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobError {
    /// La clave no existe.
    NotFound,
    /// La clave ya existe: el almacén no sobrescribe.
    AlreadyExists,
    /// La clave no cumple `[a-z0-9-]{1,64}`.
    InvalidKey,
    /// Falla de entrada/salida. Sin detalle a propósito: podría traer una ruta.
    Io,
}

impl fmt::Display for BlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotFound => "el archivo no existe",
            Self::AlreadyExists => "el archivo ya existe",
            Self::InvalidKey => "la clave del archivo no es válida",
            Self::Io => "falló el acceso al almacén de archivos",
        })
    }
}

impl std::error::Error for BlobError {}

/// Error al preparar el almacén en disco.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobConfigError {
    /// Falta la variable o está vacía. Lleva el nombre, nunca el valor.
    MissingVar(&'static str),
    /// No se pudo crear o proteger el directorio base (sin ruta).
    DirUnavailable,
}

impl fmt::Display for BlobConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingVar(var) => write!(f, "falta la variable de entorno {var}"),
            Self::DirUnavailable => write!(
                f,
                "no se pudo preparar el directorio del almacén de archivos ({BLOB_DIR_VAR})"
            ),
        }
    }
}

impl std::error::Error for BlobConfigError {}

/// Contrato del almacén de archivos.
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Guarda `bytes` bajo `key`. Falla con `AlreadyExists` si la clave ya existe.
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), BlobError>;
    /// Lee los bytes de `key`. `NotFound` si no existe.
    async fn get(&self, key: &str) -> Result<Vec<u8>, BlobError>;
    /// Borra `key`. Idempotente: borrar una clave ausente no falla.
    async fn delete(&self, key: &str) -> Result<(), BlobError>;
}

/// Un almacén en memoria nuevo, listo para ponerse en `AppState`. Pensado para pruebas.
pub fn memory_store() -> std::sync::Arc<dyn BlobStore> {
    std::sync::Arc::new(MemoryBlobStore::new())
}

/// Clave nueva: UUID v7 en minúsculas, sin nombre de archivo ni id de usuario.
pub fn new_blob_key() -> String {
    migration::new_id().to_string()
}

fn validate_key(key: &str) -> Result<(), BlobError> {
    let ok = !key.is_empty()
        && key.len() <= MAX_KEY_LEN
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(BlobError::InvalidKey)
    }
}

/// Almacén en disco privado.
pub struct LocalBlobStore {
    base: PathBuf,
}

// A mano para que un `{:?}` perdido en un log no filtre la ruta.
impl fmt::Debug for LocalBlobStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LocalBlobStore { .. }")
    }
}

impl LocalBlobStore {
    /// Crea (si hace falta) el directorio base con permisos `0700`.
    pub fn new(base: impl Into<PathBuf>) -> Result<Self, BlobConfigError> {
        let base = base.into();
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&base)
            .map_err(|_| BlobConfigError::DirUnavailable)?;
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| BlobConfigError::DirUnavailable)?;
        Ok(Self { base })
    }

    /// Carga el directorio desde un buscador de variables (`BLOB_DIR`). Existe para poder
    /// probar la carga sin tocar el entorno real del proceso.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, BlobConfigError> {
        let dir = lookup(BLOB_DIR_VAR)
            .filter(|v| !v.trim().is_empty())
            .ok_or(BlobConfigError::MissingVar(BLOB_DIR_VAR))?;
        Self::new(dir)
    }

    /// Carga el directorio desde `BLOB_DIR`. El servicio no arranca si falla.
    pub fn from_env() -> Result<Self, BlobConfigError> {
        Self::from_lookup(|nombre| std::env::var(nombre).ok())
    }
}

fn map_io(e: std::io::Error) -> BlobError {
    match e.kind() {
        std::io::ErrorKind::NotFound => BlobError::NotFound,
        std::io::ErrorKind::AlreadyExists => BlobError::AlreadyExists,
        _ => BlobError::Io,
    }
}

fn put_blocking(base: &Path, key: &str, bytes: &[u8]) -> Result<(), BlobError> {
    let destino = base.join(key);
    // Temporal con nombre que ninguna clave válida puede tener (empieza con punto).
    let tmp = base.join(format!(".tmp-{}", migration::new_id()));
    let escribir = || -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    };
    let resultado = match escribir() {
        // `hard_link` falla si el destino existe: publica de forma atómica y sin sobrescribir.
        Ok(()) => std::fs::hard_link(&tmp, &destino).map_err(map_io),
        Err(e) => Err(map_io(e)),
    };
    let _ = std::fs::remove_file(&tmp);
    resultado.map_err(|e| {
        if e == BlobError::NotFound {
            BlobError::Io
        } else {
            e
        }
    })
}

fn get_blocking(base: &Path, key: &str) -> Result<Vec<u8>, BlobError> {
    let ruta = base.join(key);
    let meta = std::fs::symlink_metadata(&ruta).map_err(map_io)?;
    if !meta.is_file() {
        return Err(BlobError::Io);
    }
    std::fs::read(&ruta).map_err(map_io)
}

fn delete_blocking(base: &Path, key: &str) -> Result<(), BlobError> {
    match std::fs::remove_file(base.join(key)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(BlobError::Io),
    }
}

#[async_trait]
impl BlobStore for LocalBlobStore {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), BlobError> {
        validate_key(key)?;
        let (base, key, bytes) = (self.base.clone(), key.to_owned(), bytes.to_vec());
        tokio::task::spawn_blocking(move || put_blocking(&base, &key, &bytes))
            .await
            .map_err(|_| BlobError::Io)?
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, BlobError> {
        validate_key(key)?;
        let (base, key) = (self.base.clone(), key.to_owned());
        tokio::task::spawn_blocking(move || get_blocking(&base, &key))
            .await
            .map_err(|_| BlobError::Io)?
    }

    async fn delete(&self, key: &str) -> Result<(), BlobError> {
        validate_key(key)?;
        let (base, key) = (self.base.clone(), key.to_owned());
        tokio::task::spawn_blocking(move || delete_blocking(&base, &key))
            .await
            .map_err(|_| BlobError::Io)?
    }
}

/// Almacén en memoria, para pruebas. Mismo contrato que [`LocalBlobStore`].
#[derive(Default)]
pub struct MemoryBlobStore {
    datos: Mutex<HashMap<String, Vec<u8>>>,
}

impl fmt::Debug for MemoryBlobStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MemoryBlobStore { .. }")
    }
}

impl MemoryBlobStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn datos(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<u8>>> {
        self.datos.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait]
impl BlobStore for MemoryBlobStore {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), BlobError> {
        validate_key(key)?;
        let mut datos = self.datos();
        if datos.contains_key(key) {
            return Err(BlobError::AlreadyExists);
        }
        datos.insert(key.to_owned(), bytes.to_vec());
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, BlobError> {
        validate_key(key)?;
        self.datos().get(key).cloned().ok_or(BlobError::NotFound)
    }

    async fn delete(&self, key: &str) -> Result<(), BlobError> {
        validate_key(key)?;
        self.datos().remove(key);
        Ok(())
    }
}

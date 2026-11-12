//! Cifrado de PII a nivel de aplicación.
//!
//! - Los datos sensibles se cifran con AES-256-GCM **antes** de tocar PostgreSQL.
//! - Una columna cifrada no se puede buscar, así que el email lleva además un HMAC-SHA256
//!   determinístico (`email_hash`) con una clave **distinta** de la de cifrado.
//!
//! Ninguna función de este módulo escribe una clave, un texto en claro o un valor de variable
//! de entorno en un mensaje de error ni en `Debug`. Ver `documentation/security.md`.

use std::fmt;

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const ENCRYPTION_KEY_VAR: &str = "PII_ENCRYPTION_KEY";
pub const HASH_KEY_VAR: &str = "PII_HASH_KEY";

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

#[derive(Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// Falta una variable de entorno. Lleva el nombre, nunca un valor.
    MissingVar(&'static str),
    /// La clave de esa variable no es base64 válido de 32 bytes.
    InvalidKey(&'static str),
    /// Las dos claves son idénticas: deben ser distintas.
    SameKeys,
    /// El dato está alterado, es muy corto o la clave no corresponde.
    DecryptionFailed,
    /// No se pudo cifrar.
    EncryptionFailed,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingVar(var) => write!(f, "falta la variable de entorno {var}"),
            Self::InvalidKey(var) => write!(
                f,
                "{var} no es una clave válida: debe ser base64 de exactamente {KEY_LEN} bytes \
                 (`openssl rand -base64 32`)"
            ),
            Self::SameKeys => write!(
                f,
                "{ENCRYPTION_KEY_VAR} y {HASH_KEY_VAR} deben ser claves distintas"
            ),
            Self::DecryptionFailed => write!(f, "no se pudo descifrar el dato"),
            Self::EncryptionFailed => write!(f, "no se pudo cifrar el dato"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// Las dos claves de PII: una para cifrar, otra para el HMAC de búsqueda.
pub struct PiiKeys {
    encryption: [u8; KEY_LEN],
    hash: [u8; KEY_LEN],
}

// A mano para que un `{:?}` perdido en un log no filtre las claves.
impl fmt::Debug for PiiKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PiiKeys { .. }")
    }
}

fn decode_key(var: &'static str, valor: &str) -> Result<[u8; KEY_LEN], CryptoError> {
    let bytes = STANDARD
        .decode(valor.trim())
        .map_err(|_| CryptoError::InvalidKey(var))?;
    bytes.try_into().map_err(|_| CryptoError::InvalidKey(var))
}

impl PiiKeys {
    /// Construye desde dos claves en base64 (cifrado, hash).
    pub fn from_base64(encryption: &str, hash: &str) -> Result<Self, CryptoError> {
        let encryption = decode_key(ENCRYPTION_KEY_VAR, encryption)?;
        let hash = decode_key(HASH_KEY_VAR, hash)?;
        if encryption == hash {
            return Err(CryptoError::SameKeys);
        }
        Ok(Self { encryption, hash })
    }

    /// Carga desde un buscador de variables. Existe para poder probar la carga sin tocar el
    /// entorno real del proceso.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, CryptoError> {
        let encryption =
            lookup(ENCRYPTION_KEY_VAR).ok_or(CryptoError::MissingVar(ENCRYPTION_KEY_VAR))?;
        let hash = lookup(HASH_KEY_VAR).ok_or(CryptoError::MissingVar(HASH_KEY_VAR))?;
        Self::from_base64(&encryption, &hash)
    }

    /// Carga desde las variables de entorno del proceso. El servicio no arranca si falla.
    pub fn from_env() -> Result<Self, CryptoError> {
        Self::from_lookup(|nombre| std::env::var(nombre).ok())
    }

    /// Cifra con AES-256-GCM. Resultado: `nonce (12 bytes) ‖ texto cifrado ‖ etiqueta (16)`.
    pub fn encrypt(&self, texto: &str) -> Result<Vec<u8>, CryptoError> {
        let cifrador = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.encryption));
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let cifrado = cifrador
            .encrypt(&nonce, texto.as_bytes())
            .map_err(|_| CryptoError::EncryptionFailed)?;

        let mut salida = Vec::with_capacity(NONCE_LEN + cifrado.len());
        salida.extend_from_slice(&nonce);
        salida.extend_from_slice(&cifrado);
        Ok(salida)
    }

    /// Descifra lo producido por [`PiiKeys::encrypt`]. Falla si el dato fue alterado.
    pub fn decrypt(&self, dato: &[u8]) -> Result<String, CryptoError> {
        if dato.len() < NONCE_LEN + TAG_LEN {
            return Err(CryptoError::DecryptionFailed);
        }
        let (nonce, cifrado) = dato.split_at(NONCE_LEN);
        let cifrador = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.encryption));
        let claro = cifrador
            .decrypt(Nonce::from_slice(nonce), cifrado)
            .map_err(|_| CryptoError::DecryptionFailed)?;
        String::from_utf8(claro).map_err(|_| CryptoError::DecryptionFailed)
    }

    /// HMAC-SHA256 con la clave de hash y un prefijo de dominio, para que el hash de un NIT o de
    /// una IP nunca coincida con el de un correo ni entre sí.
    fn domain_hash(&self, dominio: &str, dato: &str) -> Vec<u8> {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.hash)
            .expect("HMAC acepta claves de cualquier largo");
        mac.update(dominio.as_bytes());
        mac.update(&[0]);
        mac.update(dato.as_bytes());
        mac.finalize().into_bytes().to_vec()
    }

    /// HMAC del NIT **ya normalizado** (mayúsculas, sin guiones ni espacios). Determinístico:
    /// es la unicidad de `companies.tax_id_hash`.
    pub fn tax_id_hash(&self, nit_normalizado: &str) -> Vec<u8> {
        self.domain_hash("tax_id", nit_normalizado)
    }

    /// HMAC de una IP para `access_audit.ip_hash`: se puede comparar, no se puede leer.
    pub fn ip_hash(&self, ip: &str) -> Vec<u8> {
        self.domain_hash("ip", ip.trim())
    }

    /// HMAC-SHA256 del email normalizado (sin espacios en los extremos y en minúsculas).
    /// Determinístico: sirve para buscar y para la restricción de unicidad.
    pub fn email_hash(&self, email: &str) -> Vec<u8> {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.hash)
            .expect("HMAC acepta claves de cualquier largo");
        mac.update(email.trim().to_lowercase().as_bytes());
        mac.finalize().into_bytes().to_vec()
    }
}

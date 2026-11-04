//! JWT de acceso: emisión y validación (HS256, 15 minutos).
//!
//! - Claims **solo** `sub` (user_id), `iat`, `exp`, `iss`, `aud`. Nada de email, nombre ni
//!   datos personales: el JWT viaja en cada request y se puede leer (solo va firmado).
//! - El algoritmo está fijo en HS256; `alg: none` y cualquier otro se rechazan.
//! - Se validan firma, `iss`, `aud` y `exp`. Sin margen (`leeway`) de reloj.
//! - El secreto (`JWT_SECRET`) debe tener al menos 32 bytes.
//! - El reloj es inyectable: `issue_at`/`validate_at` reciben el "ahora" en segundos Unix.
//!
//! Ningún error ni `Debug` de este módulo escribe el secreto ni un token.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

pub const JWT_SECRET_VAR: &str = "JWT_SECRET";
pub const ISSUER: &str = "jobout-core-api";
pub const AUDIENCE: &str = "jobout";
/// Vida del token de acceso: 15 minutos.
pub const ACCESS_TTL_SECS: u64 = 15 * 60;

const MIN_SECRET_LEN: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub enum JwtError {
    /// Falta una variable de entorno. Lleva el nombre, nunca un valor.
    MissingVar(&'static str),
    /// El secreto de esa variable mide menos de 32 bytes.
    SecretTooShort(&'static str),
    /// Token mal formado, firmado con otra clave, de otro algoritmo, de otro emisor o
    /// audiencia, o expirado. A propósito no distingue la causa.
    Invalid,
    /// No se pudo firmar el token.
    Encoding,
}

impl fmt::Display for JwtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingVar(var) => write!(f, "falta la variable de entorno {var}"),
            Self::SecretTooShort(var) => {
                write!(
                    f,
                    "{var} debe tener al menos {MIN_SECRET_LEN} bytes (`openssl rand -base64 48`)"
                )
            }
            Self::Invalid => write!(f, "token inválido"),
            Self::Encoding => write!(f, "no se pudo firmar el token"),
        }
    }
}

impl std::error::Error for JwtError {}

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    iat: u64,
    exp: u64,
    iss: String,
    aud: String,
}

/// Secreto de firma de los JWT de acceso.
pub struct JwtKeys {
    secret: Vec<u8>,
}

// A mano para que un `{:?}` perdido en un log no filtre el secreto.
impl fmt::Debug for JwtKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JwtKeys { .. }")
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl JwtKeys {
    /// Construye desde el secreto. Exige al menos 32 bytes.
    pub fn from_secret(secret: &str) -> Result<Self, JwtError> {
        if secret.len() < MIN_SECRET_LEN {
            return Err(JwtError::SecretTooShort(JWT_SECRET_VAR));
        }
        Ok(Self {
            secret: secret.as_bytes().to_vec(),
        })
    }

    /// Carga desde un buscador de variables. Existe para poder probar la carga sin tocar el
    /// entorno real del proceso.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, JwtError> {
        let secret = lookup(JWT_SECRET_VAR).ok_or(JwtError::MissingVar(JWT_SECRET_VAR))?;
        Self::from_secret(&secret)
    }

    /// Carga desde las variables de entorno del proceso. El servicio no arranca si falla.
    pub fn from_env() -> Result<Self, JwtError> {
        Self::from_lookup(|nombre| std::env::var(nombre).ok())
    }

    /// Emite un token de acceso para `user_id` con el reloj real.
    pub fn issue(&self, user_id: &str) -> Result<String, JwtError> {
        self.issue_at(user_id, now_secs())
    }

    /// Emite un token como si `now` (segundos Unix) fuera el momento actual.
    pub fn issue_at(&self, user_id: &str, now: u64) -> Result<String, JwtError> {
        let claims = Claims {
            sub: user_id.to_owned(),
            iat: now,
            exp: now + ACCESS_TTL_SECS,
            iss: ISSUER.to_owned(),
            aud: AUDIENCE.to_owned(),
        };
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(&self.secret),
        )
        .map_err(|_| JwtError::Encoding)
    }

    /// Valida un token con el reloj real y devuelve el `user_id`.
    pub fn validate(&self, token: &str) -> Result<String, JwtError> {
        self.validate_at(token, now_secs())
    }

    /// Valida un token como si `now` (segundos Unix) fuera el momento actual.
    pub fn validate_at(&self, token: &str, now: u64) -> Result<String, JwtError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_issuer(&[ISSUER]);
        validation.set_audience(&[AUDIENCE]);
        validation.set_required_spec_claims(&["sub", "exp", "iss", "aud"]);
        validation.leeway = 0;
        // `exp` se compara abajo contra el reloj inyectado, no contra el del sistema.
        validation.validate_exp = false;

        let data = decode::<Claims>(token, &DecodingKey::from_secret(&self.secret), &validation)
            .map_err(|_| JwtError::Invalid)?;
        if data.claims.exp <= now {
            return Err(JwtError::Invalid);
        }
        Ok(data.claims.sub)
    }
}

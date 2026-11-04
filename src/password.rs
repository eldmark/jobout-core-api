//! Contraseñas: política, hash y verificación con Argon2id.
//!
//! - Hash Argon2id con sal aleatoria por contraseña (formato PHC: la sal y los parámetros
//!   viajan dentro de la cadena, no hace falta columna aparte).
//! - Parámetros: los mínimos recomendados por OWASP para Argon2id, `m = 19 MiB`, `t = 2`,
//!   `p = 1`. Equilibran costo para un atacante y latencia de un login (decenas de ms).
//!   Subirlos más adelante es compatible: cada hash guarda los suyos.
//! - Política: de 10 a 128 caracteres. El tope evita que alguien mande megabytes de
//!   "contraseña" para quemar CPU; no se exige composición (mayúsculas, símbolos).
//!
//! Ninguna función de este módulo escribe una contraseña ni un hash en un mensaje de error.

use std::fmt;
use std::sync::OnceLock;

use argon2::{
    password_hash::{PasswordHasher, PasswordVerifier},
    Algorithm, Argon2, Params, Version,
};

/// Largo mínimo de una contraseña, en caracteres.
pub const MIN_LEN: usize = 10;
/// Largo máximo de una contraseña, en caracteres.
pub const MAX_LEN: usize = 128;

/// Memoria en KiB (19 MiB), pasadas y paralelismo de Argon2id.
const MEMORY_KIB: u32 = 19 * 1024;
const ITERATIONS: u32 = 2;
const PARALLELISM: u32 = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum PasswordError {
    /// Menos de [`MIN_LEN`] caracteres.
    TooShort,
    /// Más de [`MAX_LEN`] caracteres.
    TooLong,
    /// Argon2 falló (no debería pasar con parámetros válidos).
    HashingFailed,
}

impl fmt::Display for PasswordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => write!(f, "la contraseña debe tener al menos {MIN_LEN} caracteres"),
            Self::TooLong => write!(f, "la contraseña no puede pasar de {MAX_LEN} caracteres"),
            Self::HashingFailed => write!(f, "no se pudo procesar la contraseña"),
        }
    }
}

impl std::error::Error for PasswordError {}

fn argon2() -> Argon2<'static> {
    let params = Params::new(MEMORY_KIB, ITERATIONS, PARALLELISM, None)
        .expect("los parámetros de Argon2 son constantes válidas");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Valida la política de largo (en caracteres, no en bytes).
pub fn validate_policy(password: &str) -> Result<(), PasswordError> {
    let largo = password.chars().count();
    if largo < MIN_LEN {
        Err(PasswordError::TooShort)
    } else if largo > MAX_LEN {
        Err(PasswordError::TooLong)
    } else {
        Ok(())
    }
}

/// Valida la política y devuelve el hash Argon2id en formato PHC (`$argon2id$...`).
pub fn hash_password(password: &str) -> Result<String, PasswordError> {
    validate_policy(password)?;
    argon2()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|_| PasswordError::HashingFailed)
}

/// `true` solo si la contraseña corresponde al hash. Un hash ilegible da `false`.
///
/// Una contraseña fuera de política no puede corresponder a ningún hash que emitimos, así que
/// se descarta sin correr Argon2 (evita que un texto enorme gaste CPU).
pub fn verify_password(password: &str, hash: &str) -> bool {
    if validate_policy(password).is_err() {
        return false;
    }
    argon2().verify_password(password.as_bytes(), hash).is_ok()
}

/// Hash ficticio fijo, con los mismos parámetros que uno real. Se calcula una vez.
pub fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        argon2()
            .hash_password(b"jobout-dummy-password-never-matches")
            .expect("hash ficticio")
            .to_string()
    })
}

/// Para cuando el usuario no existe: corre una verificación Argon2 completa contra el hash
/// ficticio para que responder "no existe" tarde lo mismo que "contraseña incorrecta".
/// Siempre devuelve `false`.
pub fn verify_dummy(password: &str) -> bool {
    // Se ejecuta aunque la contraseña esté fuera de política: no hay atajo de tiempo.
    let _ = argon2().verify_password(password.as_bytes(), dummy_hash());
    false
}

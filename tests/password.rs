//! Casos de verdad de contraseñas (P1–P5 y R9 de docs/working/fase1-identidad.md).
//!
//! Se escribieron antes que la implementación, en rojo. Ver CONTRIBUTING.md.

use std::time::{Duration, Instant};

use jobout_core_api::password::{
    dummy_hash, hash_password, validate_policy, verify_dummy, verify_password, PasswordError,
};

const BUENA: &str = "correcta-1234";

/// P1: el hash es una cadena Argon2id, no la contraseña.
#[test]
fn el_hash_es_argon2id_y_no_contiene_la_contrasena() {
    let hash = hash_password(BUENA).unwrap();
    assert!(hash.starts_with("$argon2id$"));
    assert!(!hash.contains(BUENA));
}

/// P2: la contraseña correcta verifica; una incorrecta, no.
#[test]
fn la_correcta_verifica_y_la_incorrecta_no() {
    let hash = hash_password(BUENA).unwrap();
    assert!(verify_password(BUENA, &hash));
    assert!(!verify_password("incorrecta-1234", &hash));
}

/// Un hash corrupto o vacío nunca verifica (y no hace panic).
#[test]
fn un_hash_invalido_no_verifica() {
    assert!(!verify_password(BUENA, ""));
    assert!(!verify_password(BUENA, "no-es-un-hash"));
}

/// P3: el mismo texto hasheado dos veces da hashes distintos (sal aleatoria).
#[test]
fn el_mismo_texto_da_hashes_distintos() {
    let a = hash_password(BUENA).unwrap();
    let b = hash_password(BUENA).unwrap();
    assert_ne!(a, b);
    assert!(verify_password(BUENA, &a) && verify_password(BUENA, &b));
}

/// P4: menos de 10 caracteres se rechaza; 10 se acepta.
#[test]
fn menos_de_10_caracteres_se_rechaza_y_10_se_acepta() {
    assert_eq!(hash_password(&"a".repeat(9)), Err(PasswordError::TooShort));
    assert_eq!(
        validate_policy(&"a".repeat(9)),
        Err(PasswordError::TooShort)
    );
    assert_eq!(validate_policy(&"a".repeat(10)), Ok(()));
    assert!(hash_password(&"a".repeat(10)).is_ok());
}

/// P4: el largo se cuenta en caracteres, no en bytes (la ñ ocupa 2 bytes).
#[test]
fn el_largo_cuenta_caracteres_no_bytes() {
    assert_eq!(
        validate_policy(&"ñ".repeat(9)),
        Err(PasswordError::TooShort)
    );
    assert_eq!(validate_policy(&"ñ".repeat(10)), Ok(()));
    assert_eq!(validate_policy(&"ñ".repeat(128)), Ok(()));
}

/// P5: más de 128 caracteres se rechaza; 128 se acepta.
#[test]
fn mas_de_128_caracteres_se_rechaza_y_128_se_acepta() {
    assert_eq!(validate_policy(&"a".repeat(128)), Ok(()));
    assert_eq!(
        validate_policy(&"a".repeat(129)),
        Err(PasswordError::TooLong)
    );
    assert_eq!(hash_password(&"a".repeat(129)), Err(PasswordError::TooLong));
}

/// P5: verificar una contraseña enorme falla sin gastar CPU en Argon2.
#[test]
fn verificar_una_contrasena_enorme_falla_rapido() {
    let hash = hash_password(BUENA).unwrap();
    let t = Instant::now();
    assert!(!verify_password(&"a".repeat(100_000), &hash));
    assert!(t.elapsed() < Duration::from_millis(50));
}

/// R9: el hash ficticio es Argon2id con los mismos parámetros que un hash real.
#[test]
fn el_hash_ficticio_tiene_los_mismos_parametros_que_uno_real() {
    let real = hash_password(BUENA).unwrap();
    let parametros = |h: &str| h.split('$').nth(3).map(str::to_owned);
    assert!(dummy_hash().starts_with("$argon2id$"));
    assert!(parametros(&real).is_some());
    assert_eq!(parametros(dummy_hash()), parametros(&real));
}

/// R9: verificar contra el ficticio devuelve siempre falso, incluso con su propio texto.
#[test]
fn verificar_contra_el_ficticio_siempre_es_falso() {
    assert!(!verify_dummy(BUENA));
    assert!(!verify_dummy(""));
}

/// R9: verificar contra el ficticio hace trabajo Argon2 real (no hay atajo de tiempo).
#[test]
fn el_ficticio_cuesta_lo_mismo_que_una_verificacion_real() {
    let hash = hash_password(BUENA).unwrap();
    let mejor = |f: &dyn Fn()| {
        (0..3)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed()
            })
            .min()
            .unwrap()
    };
    let real = mejor(&|| {
        verify_password(BUENA, &hash);
    });
    let ficticio = mejor(&|| {
        verify_dummy(BUENA);
    });
    // Cota holgada para no ser frágil: un atajo costaría microsegundos, no milisegundos.
    assert!(
        ficticio * 3 > real,
        "ficticio {ficticio:?} vs real {real:?}"
    );
}

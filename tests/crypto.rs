//! Casos de verdad del cifrado de PII (C1–C13 de docs/working/fase1-identidad.md).
//!
//! Se escribieron antes que la implementación, en rojo. Ver CONTRIBUTING.md.

use base64::{engine::general_purpose::STANDARD, Engine};
use jobout_core_api::crypto::{CryptoError, PiiKeys};

fn clave(byte: u8) -> String {
    STANDARD.encode([byte; 32])
}

fn llaves() -> PiiKeys {
    PiiKeys::from_base64(&clave(1), &clave(2)).unwrap()
}

/// C1: cifrar y descifrar con la misma clave devuelve el original.
#[test]
fn cifrar_y_descifrar_devuelve_el_original() {
    let k = llaves();
    let cifrado = k.encrypt("ana@mail.com").unwrap();
    assert_eq!(k.decrypt(&cifrado).unwrap(), "ana@mail.com");
}

/// C2: dos cifrados del mismo texto son distintos (nonce aleatorio).
#[test]
fn cifrar_dos_veces_da_resultados_distintos() {
    let k = llaves();
    assert_ne!(
        k.encrypt("ana@mail.com").unwrap(),
        k.encrypt("ana@mail.com").unwrap()
    );
}

/// El cifrado no deja el texto en claro dentro del resultado.
#[test]
fn el_resultado_no_contiene_el_texto_en_claro() {
    let cifrado = llaves().encrypt("ana@mail.com").unwrap();
    let texto = String::from_utf8_lossy(&cifrado);
    assert!(!texto.contains("ana@mail.com"));
}

/// C3: descifrar con otra clave falla.
#[test]
fn descifrar_con_otra_clave_falla() {
    let cifrado = llaves().encrypt("secreto").unwrap();
    let otra = PiiKeys::from_base64(&clave(9), &clave(2)).unwrap();
    assert!(matches!(
        otra.decrypt(&cifrado),
        Err(CryptoError::DecryptionFailed)
    ));
}

/// C4: un byte alterado se detecta (autenticación GCM).
#[test]
fn un_byte_alterado_se_detecta() {
    let k = llaves();
    let mut cifrado = k.encrypt("secreto").unwrap();
    let ultimo = cifrado.len() - 1;
    cifrado[ultimo] ^= 0x01;
    assert!(matches!(
        k.decrypt(&cifrado),
        Err(CryptoError::DecryptionFailed)
    ));
}

/// C5: el hash del email es determinístico.
#[test]
fn email_hash_es_deterministico() {
    let k = llaves();
    assert_eq!(k.email_hash("ana@mail.com"), k.email_hash("ana@mail.com"));
}

/// C6: el email se normaliza antes de hashear.
#[test]
fn email_hash_normaliza_mayusculas_y_espacios() {
    let k = llaves();
    assert_eq!(k.email_hash(" Ana@Mail.COM "), k.email_hash("ana@mail.com"));
}

/// Emails distintos dan hashes distintos.
#[test]
fn emails_distintos_dan_hashes_distintos() {
    let k = llaves();
    assert_ne!(k.email_hash("ana@mail.com"), k.email_hash("luis@mail.com"));
}

/// C7: otra clave de HMAC da otro hash.
#[test]
fn email_hash_depende_de_la_clave() {
    let a = PiiKeys::from_base64(&clave(1), &clave(2)).unwrap();
    let b = PiiKeys::from_base64(&clave(1), &clave(3)).unwrap();
    assert_ne!(a.email_hash("ana@mail.com"), b.email_hash("ana@mail.com"));
}

/// C8: si falta una variable, el error la nombra pero no imprime ninguna clave.
#[test]
fn falta_una_variable_se_nombra_sin_imprimir_claves() {
    let valor = clave(7);
    let lookup = |nombre: &str| match nombre {
        "PII_ENCRYPTION_KEY" => Some(valor.clone()),
        _ => None,
    };
    let error = PiiKeys::from_lookup(lookup).unwrap_err();
    let mensaje = error.to_string();
    assert!(mensaje.contains("PII_HASH_KEY"), "mensaje: {mensaje}");
    assert!(!mensaje.contains(&valor), "el error imprimió una clave");

    let sin_ninguna = PiiKeys::from_lookup(|_| None).unwrap_err().to_string();
    assert!(sin_ninguna.contains("PII_ENCRYPTION_KEY"));
}

/// C8 (camino feliz): con las dos variables, carga.
#[test]
fn con_las_dos_variables_carga() {
    let (a, b) = (clave(1), clave(2));
    let k = PiiKeys::from_lookup(|n| match n {
        "PII_ENCRYPTION_KEY" => Some(a.clone()),
        "PII_HASH_KEY" => Some(b.clone()),
        _ => None,
    })
    .unwrap();
    assert_eq!(k.decrypt(&k.encrypt("x").unwrap()).unwrap(), "x");
}

/// C9: base64 inválido o largo distinto de 32 bytes se rechaza.
#[test]
fn clave_invalida_se_rechaza() {
    let corta = STANDARD.encode([1u8; 16]);
    let larga = STANDARD.encode([1u8; 33]);
    for mala in ["no es base64 !!!", corta.as_str(), larga.as_str(), ""] {
        assert!(
            PiiKeys::from_base64(mala, &clave(2)).is_err(),
            "aceptó la clave de cifrado {mala:?}"
        );
        assert!(
            PiiKeys::from_base64(&clave(1), mala).is_err(),
            "aceptó la clave de hash {mala:?}"
        );
    }
}

/// C9: el error de clave inválida no repite el valor recibido.
#[test]
fn el_error_de_clave_invalida_no_repite_el_valor() {
    let mala = "valor-super-secreto-que-no-es-base64!!";
    let mensaje = PiiKeys::from_base64(mala, &clave(2))
        .unwrap_err()
        .to_string();
    assert!(!mensaje.contains(mala));
    assert!(mensaje.contains("PII_ENCRYPTION_KEY"));
}

/// C10: las dos claves deben ser distintas.
#[test]
fn claves_iguales_se_rechazan() {
    assert!(matches!(
        PiiKeys::from_base64(&clave(5), &clave(5)),
        Err(CryptoError::SameKeys)
    ));
}

/// C11: un dato demasiado corto da error, no `panic`.
#[test]
fn dato_demasiado_corto_da_error() {
    let k = llaves();
    for largo in [0usize, 1, 11, 12, 20] {
        assert!(
            k.decrypt(&vec![0u8; largo]).is_err(),
            "largo {largo} no falló"
        );
    }
}

/// C12: texto con tildes, ñ y emoji vuelve idéntico.
#[test]
fn texto_unicode_vuelve_identico() {
    let k = llaves();
    let texto = "Peña Núñez — Guatemala 🇬🇹 ¿qué tal?";
    assert_eq!(k.decrypt(&k.encrypt(texto).unwrap()).unwrap(), texto);
}

/// C13: `Debug` nunca imprime las claves (un `{:?}` perdido en un log no las filtra).
#[test]
fn debug_no_imprime_las_claves() {
    let k = llaves();
    let salida = format!("{k:?}");
    assert!(!salida.contains(&clave(1)));
    assert!(!salida.contains(&clave(2)));
    assert!(salida.contains("PiiKeys"));
}

/// Los hashes de NIT y de IP son determinísticos, de 32 bytes y de dominios separados: el HMAC
/// de un NIT nunca coincide con el de un correo, una IP o un NIT distinto.
#[test]
fn los_hashes_de_nit_e_ip_son_deterministicos_y_de_dominios_separados() {
    let k = llaves();
    assert_eq!(k.tax_id_hash("12345678"), k.tax_id_hash("12345678"));
    assert_ne!(k.tax_id_hash("12345678"), k.tax_id_hash("12345679"));
    assert_eq!(k.tax_id_hash("12345678").len(), 32);
    assert_eq!(k.ip_hash(" 203.0.113.7 "), k.ip_hash("203.0.113.7"));
    assert_ne!(k.tax_id_hash("abc"), k.email_hash("abc"));
    assert_ne!(k.tax_id_hash("abc"), k.ip_hash("abc"));
    assert_ne!(k.ip_hash("abc"), k.email_hash("abc"));
    // Otra clave de hash, otro resultado.
    let otra = PiiKeys::from_base64(&clave(1), &clave(3)).unwrap();
    assert_ne!(k.tax_id_hash("12345678"), otra.tax_id_hash("12345678"));
}

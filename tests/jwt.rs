//! Casos de verdad del JWT de acceso (J1–J8 de docs/working/fase1-identidad.md).
//!
//! Se escribieron antes que la implementación, en rojo. Ver CONTRIBUTING.md.
//! El reloj se inyecta con `issue_at`/`validate_at`: ningún test duerme.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jobout_core_api::jwt::{JwtError, JwtKeys, ACCESS_TTL_SECS, AUDIENCE, ISSUER};
use serde_json::{json, Value};

const SECRETO: &str = "0123456789abcdef0123456789abcdef";
const OTRO: &str = "fedcba9876543210fedcba9876543210";
const USER: &str = "7b0f6c1e-4a52-4a4e-9a55-0d4f1c2b3a10";
const AHORA: u64 = 1_800_000_000;

fn llaves() -> JwtKeys {
    JwtKeys::from_secret(SECRETO).unwrap()
}

fn codificar(v: &Value) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).unwrap())
}

fn payload(token: &str) -> Value {
    let medio = token.split('.').nth(1).unwrap();
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(medio).unwrap()).unwrap()
}

/// Firma a mano un token HS256 con claims arbitrarios (para forjar casos).
fn firmar(secreto: &str, claims: &Value) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        claims,
        &jsonwebtoken::EncodingKey::from_secret(secreto.as_bytes()),
    )
    .unwrap()
}

fn claims_validos() -> Value {
    json!({"sub": USER, "iat": AHORA, "exp": AHORA + 900, "iss": ISSUER, "aud": AUDIENCE})
}

/// J1: un token emitido se valida y devuelve el user_id.
#[test]
fn un_token_emitido_se_valida_y_devuelve_el_user_id() {
    let k = llaves();
    let token = k.issue_at(USER, AHORA).unwrap();
    assert_eq!(k.validate_at(&token, AHORA + 1).unwrap(), USER);
}

/// J1: con el reloj real también funciona.
#[test]
fn con_el_reloj_real_tambien_funciona() {
    let k = llaves();
    let token = k.issue(USER).unwrap();
    assert_eq!(k.validate(&token).unwrap(), USER);
}

/// El token de control forjado a mano es válido (si no, los rechazos de abajo no prueban nada).
#[test]
fn el_token_de_control_forjado_es_valido() {
    let token = firmar(SECRETO, &claims_validos());
    assert_eq!(llaves().validate_at(&token, AHORA).unwrap(), USER);
}

/// J2: un token firmado con otra clave se rechaza.
#[test]
fn un_token_firmado_con_otra_clave_se_rechaza() {
    let token = JwtKeys::from_secret(OTRO)
        .unwrap()
        .issue_at(USER, AHORA)
        .unwrap();
    assert_eq!(llaves().validate_at(&token, AHORA), Err(JwtError::Invalid));
}

/// J3: un token expirado se rechaza.
#[test]
fn un_token_expirado_se_rechaza() {
    let k = llaves();
    let token = k.issue_at(USER, AHORA).unwrap();
    assert_eq!(
        k.validate_at(&token, AHORA + ACCESS_TTL_SECS + 3600),
        Err(JwtError::Invalid)
    );
}

/// J4: `alg: none` se rechaza.
#[test]
fn alg_none_se_rechaza() {
    let h = codificar(&json!({"alg": "none", "typ": "JWT"}));
    let c = codificar(&claims_validos());
    for token in [format!("{h}.{c}."), format!("{h}.{c}.abc")] {
        assert_eq!(llaves().validate_at(&token, AHORA), Err(JwtError::Invalid));
    }
}

/// J4: otro algoritmo HMAC firmado con el mismo secreto también se rechaza (algoritmo fijo).
#[test]
fn otro_algoritmo_se_rechaza() {
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS512),
        &claims_validos(),
        &jsonwebtoken::EncodingKey::from_secret(SECRETO.as_bytes()),
    )
    .unwrap();
    assert_eq!(llaves().validate_at(&token, AHORA), Err(JwtError::Invalid));
}

/// J4: basura que no es un JWT se rechaza sin panic.
#[test]
fn basura_se_rechaza() {
    for t in ["", "abc", "a.b.c", "...", "a.b"] {
        assert_eq!(llaves().validate_at(t, AHORA), Err(JwtError::Invalid));
    }
}

/// J5: `iss` distinto se rechaza.
#[test]
fn iss_distinto_se_rechaza() {
    let mut c = claims_validos();
    c["iss"] = json!("otro-emisor");
    assert_eq!(
        llaves().validate_at(&firmar(SECRETO, &c), AHORA),
        Err(JwtError::Invalid)
    );
}

/// J5: `aud` distinto se rechaza.
#[test]
fn aud_distinto_se_rechaza() {
    let mut c = claims_validos();
    c["aud"] = json!("otra-audiencia");
    assert_eq!(
        llaves().validate_at(&firmar(SECRETO, &c), AHORA),
        Err(JwtError::Invalid)
    );
}

/// J5: un token sin `iss` o sin `aud` se rechaza.
#[test]
fn sin_iss_o_sin_aud_se_rechaza() {
    for campo in ["iss", "aud", "sub", "exp"] {
        let mut c = claims_validos();
        c.as_object_mut().unwrap().remove(campo);
        assert_eq!(
            llaves().validate_at(&firmar(SECRETO, &c), AHORA),
            Err(JwtError::Invalid),
            "sin {campo}"
        );
    }
}

/// J6: el token expira a los 15 minutos exactos de emitido.
#[test]
fn el_token_expira_a_los_15_minutos() {
    let k = llaves();
    assert_eq!(ACCESS_TTL_SECS, 15 * 60);
    let token = k.issue_at(USER, AHORA).unwrap();
    let p = payload(&token);
    assert_eq!(p["exp"].as_u64().unwrap() - p["iat"].as_u64().unwrap(), 900);
    assert!(k.validate_at(&token, AHORA + 899).is_ok());
    assert_eq!(k.validate_at(&token, AHORA + 900), Err(JwtError::Invalid));
}

/// J7: un secreto de menos de 32 bytes se rechaza; uno de 32 se acepta.
#[test]
fn un_secreto_corto_se_rechaza() {
    assert!(matches!(
        JwtKeys::from_secret(&"a".repeat(31)),
        Err(JwtError::SecretTooShort(_))
    ));
    assert!(JwtKeys::from_secret(&"a".repeat(32)).is_ok());
}

/// J7: si falta la variable, el servicio no arranca y el error nombra la variable.
#[test]
fn si_falta_el_secreto_falla_nombrando_la_variable() {
    let err = JwtKeys::from_lookup(|_| None).unwrap_err();
    assert_eq!(err, JwtError::MissingVar("JWT_SECRET"));
    assert!(err.to_string().contains("JWT_SECRET"));
}

/// J7: se carga de `JWT_SECRET` y solo de esa variable.
#[test]
fn se_carga_de_jwt_secret() {
    let k = JwtKeys::from_lookup(|n| (n == "JWT_SECRET").then(|| SECRETO.to_string())).unwrap();
    let token = k.issue_at(USER, AHORA).unwrap();
    assert!(llaves().validate_at(&token, AHORA).is_ok());
}

/// J7: el error de un secreto corto nombra la variable pero jamás imprime su valor.
#[test]
fn el_error_no_imprime_el_valor_del_secreto() {
    let corto = "secreto-corto-visible";
    let err = JwtKeys::from_lookup(|_| Some(corto.to_string())).unwrap_err();
    let texto = format!("{err} / {err:?}");
    assert!(texto.contains("JWT_SECRET"));
    assert!(!texto.contains(corto));
}

/// `Debug` de las llaves está redactado: no filtra el secreto.
#[test]
fn debug_de_las_llaves_esta_redactado() {
    let texto = format!("{:?}", llaves());
    assert!(!texto.contains(SECRETO));
    assert_eq!(texto, "JwtKeys { .. }");
}

/// J8: los claims son exactamente sub, iat, exp, iss, aud; nada personal.
#[test]
fn los_claims_son_solo_los_cinco_permitidos() {
    let token = llaves().issue_at(USER, AHORA).unwrap();
    let p = payload(&token);
    let mut campos: Vec<&str> = p.as_object().unwrap().keys().map(String::as_str).collect();
    campos.sort_unstable();
    assert_eq!(campos, ["aud", "exp", "iat", "iss", "sub"]);
    assert_eq!(p["sub"], USER);
    assert_eq!(p["iss"], ISSUER);
    assert_eq!(p["aud"], AUDIENCE);
    let texto = token.to_lowercase() + &p.to_string().to_lowercase();
    assert!(!texto.contains("email") && !texto.contains('@'));
}

//! El cuerpo de `POST /offers/{id}/applications`: consentimiento y `shared_fields`.
//!
//! `shared_fields` son **nombres** de un conjunto cerrado, nunca valores. Los mensajes de error
//! son textos fijos: jamás repiten lo recibido.

/// Los diez nombres del contrato, en el orden canónico en que se guardan.
pub const FIELDS: [&str; 10] = [
    "display_name",
    "skills",
    "experience",
    "experience_descriptions",
    "languages",
    "education",
    "profile_description",
    "email",
    "phone",
    "match_score",
];

/// Lo que siempre va: sin ello la aplicación se reduciría a un nombre.
pub const MANDATORY: [&str; 3] = ["display_name", "skills", "experience"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyInput {
    /// En el orden canónico de [`FIELDS`], sin repetidos.
    pub shared_fields: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyError {
    /// 422 `consentimiento_requerido`.
    Consent,
    /// 422 `campo_no_permitido`.
    UnknownField,
    /// 422 `datos_invalidos`, con texto fijo.
    Invalid(&'static str),
}

/// Valida el cuerpo. Orden: objeto JSON → claves conocidas → `consent` es el booleano `true` →
/// `shared_fields` válidos.
pub fn parse_apply(cuerpo: &[u8]) -> Result<ApplyInput, ApplyError> {
    const FORMATO: ApplyError = ApplyError::Invalid("El cuerpo no tiene el formato esperado.");
    let valor: serde_json::Value = serde_json::from_slice(cuerpo).map_err(|_| FORMATO)?;
    let objeto = valor.as_object().ok_or(FORMATO)?;
    if objeto
        .keys()
        .any(|k| k != "consent" && k != "shared_fields")
    {
        return Err(ApplyError::UnknownField);
    }
    // El consentimiento es el booleano `true` y nada más: se comprueba antes que lo demás.
    if objeto.get("consent") != Some(&serde_json::Value::Bool(true)) {
        return Err(ApplyError::Consent);
    }
    let lista = objeto
        .get("shared_fields")
        .and_then(|v| v.as_array())
        .ok_or(ApplyError::Invalid(
            "shared_fields debe ser una lista de nombres.",
        ))?;
    if lista.len() > FIELDS.len() {
        return Err(ApplyError::Invalid(
            "shared_fields trae demasiados elementos.",
        ));
    }
    let mut marcados = [false; FIELDS.len()];
    for v in lista {
        let nombre = v.as_str().ok_or(ApplyError::Invalid(
            "shared_fields solo admite nombres de campo.",
        ))?;
        let i = FIELDS
            .iter()
            .position(|f| *f == nombre)
            .ok_or(ApplyError::Invalid(
                "shared_fields trae un campo que no se puede compartir.",
            ))?;
        if std::mem::replace(&mut marcados[i], true) {
            return Err(ApplyError::Invalid("shared_fields repite un campo."));
        }
    }
    if MANDATORY.iter().any(|m| {
        !FIELDS
            .iter()
            .position(|f| f == m)
            .is_some_and(|i| marcados[i])
    }) {
        return Err(ApplyError::Invalid(
            "shared_fields debe incluir el nombre, las habilidades y la experiencia.",
        ));
    }
    Ok(ApplyInput {
        shared_fields: FIELDS
            .iter()
            .zip(marcados)
            .filter_map(|(f, on)| on.then_some(*f))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn p(v: Value) -> Result<ApplyInput, ApplyError> {
        parse_apply(v.to_string().as_bytes())
    }

    fn minimo() -> Value {
        json!(["display_name", "skills", "experience"])
    }

    #[test]
    fn el_conjunto_cerrado_son_diez_nombres_y_tres_obligatorios() {
        assert_eq!(FIELDS.len(), 10);
        assert!(FIELDS.contains(&"match_score"));
        assert!(!MANDATORY.contains(&"match_score"));
        assert_eq!(MANDATORY, ["display_name", "skills", "experience"]);
        for m in MANDATORY {
            assert!(FIELDS.contains(&m));
        }
        // Nada de datos que el contrato prohíbe compartir, marque lo que marque la persona.
        for prohibido in [
            "cv",
            "address",
            "document",
            "birth_date",
            "age",
            "gender",
            "marital_status",
            "married_last_name",
            "second_last_name",
            "photo",
            "transcripts",
            "interview_score",
            "plan",
            "user_id",
            "applicant_id",
        ] {
            assert!(!FIELDS.contains(&prohibido), "{prohibido}");
        }
    }

    #[test]
    fn lo_minimo_con_consentimiento_pasa() {
        let r = p(json!({"consent": true, "shared_fields": minimo()})).unwrap();
        assert_eq!(r.shared_fields, ["display_name", "skills", "experience"]);
    }

    #[test]
    fn se_guarda_en_orden_canonico_aunque_llegue_desordenado() {
        let r = p(json!({"consent": true, "shared_fields":
            ["phone", "experience", "email", "skills", "display_name"]}))
        .unwrap();
        assert_eq!(
            r.shared_fields,
            ["display_name", "skills", "experience", "email", "phone"]
        );
    }

    #[test]
    fn los_nueve_pasan() {
        let r = p(json!({"consent": true, "shared_fields": FIELDS})).unwrap();
        assert_eq!(r.shared_fields, FIELDS);
    }

    #[test]
    fn el_consentimiento_es_el_booleano_true_y_nada_mas() {
        for malo in [
            json!({"shared_fields": minimo()}),
            json!({"consent": false, "shared_fields": minimo()}),
            json!({"consent": null, "shared_fields": minimo()}),
            json!({"consent": "true", "shared_fields": minimo()}),
            json!({"consent": 1, "shared_fields": minimo()}),
            json!({"consent": [true], "shared_fields": minimo()}),
            json!({"consent": {}, "shared_fields": minimo()}),
        ] {
            assert_eq!(p(malo.clone()), Err(ApplyError::Consent), "{malo}");
        }
    }

    /// Sin consentimiento gana sobre cualquier otro error del cuerpo (se comprueba primero).
    #[test]
    fn sin_consentimiento_gana_a_shared_fields_invalidos() {
        assert_eq!(
            p(json!({"consent": false, "shared_fields": ["cv"]})),
            Err(ApplyError::Consent)
        );
        assert_eq!(p(json!({"shared_fields": 5})), Err(ApplyError::Consent));
    }

    #[test]
    fn nombres_fuera_del_conjunto_repetidos_o_mal_tipados() {
        let con = |f: Value| p(json!({"consent": true, "shared_fields": f}));
        let invalido = |r: Result<ApplyInput, ApplyError>| {
            assert!(matches!(r, Err(ApplyError::Invalid(_))), "{r:?}")
        };
        invalido(con(json!(["display_name", "skills", "experience", "cv"])));
        invalido(con(json!([
            "display_name",
            "skills",
            "experience",
            "birth_date"
        ])));
        invalido(con(json!([
            "display_name",
            "skills",
            "experience",
            "Email"
        ])));
        invalido(con(json!([
            "display_name",
            "skills",
            "experience",
            "interview_score"
        ])));
        invalido(con(json!([
            "display_name",
            "display_name",
            "skills",
            "experience"
        ])));
        invalido(con(json!(["display_name", "skills", "experience", 5])));
        invalido(con(json!(["display_name", "skills", "experience", null])));
        invalido(con(json!([
            "display_name",
            "skills",
            "experience",
            ["email"]
        ])));
        invalido(con(json!("display_name")));
        invalido(con(json!({"display_name": true})));
        invalido(con(json!(null)));
        invalido(con(json!([])));
    }

    #[test]
    fn faltan_obligatorios() {
        for faltante in 0..3 {
            let mut f: Vec<&str> = MANDATORY.to_vec();
            f.remove(faltante);
            f.push("email");
            let r = p(json!({"consent": true, "shared_fields": f}));
            assert!(matches!(r, Err(ApplyError::Invalid(_))), "{f:?}: {r:?}");
        }
    }

    #[test]
    fn shared_fields_ausente_es_dato_invalido() {
        assert!(matches!(
            p(json!({"consent": true})),
            Err(ApplyError::Invalid(_))
        ));
    }

    #[test]
    fn mas_de_nueve_elementos() {
        let mut f: Vec<&str> = FIELDS.to_vec();
        f.push("display_name");
        assert!(matches!(
            p(json!({"consent": true, "shared_fields": f})),
            Err(ApplyError::Invalid(_))
        ));
    }

    #[test]
    fn claves_de_mas_en_el_cuerpo() {
        for extra in [
            "applicant_id",
            "consented_at",
            "status",
            "job_offer_id",
            "cv",
        ] {
            let mut c = json!({"consent": true, "shared_fields": minimo()});
            c[extra] = json!("x");
            assert_eq!(p(c), Err(ApplyError::UnknownField), "{extra}");
        }
    }

    #[test]
    fn cuerpo_que_no_es_un_objeto_json() {
        for malo in [
            &b""[..],
            b"no es json",
            b"[]",
            b"null",
            b"true",
            b"\"x\"",
            b"{",
        ] {
            assert!(
                matches!(parse_apply(malo), Err(ApplyError::Invalid(_))),
                "{:?}",
                String::from_utf8_lossy(malo)
            );
        }
    }

    /// Los errores no repiten lo recibido.
    #[test]
    fn los_mensajes_no_citan_lo_enviado() {
        let r = p(json!({"consent": true, "shared_fields":
            ["display_name", "skills", "experience", "secreto-ZZ"]}));
        if let Err(ApplyError::Invalid(m)) = r {
            assert!(!m.contains("secreto-ZZ"));
        } else {
            panic!("{r:?}");
        }
    }
}

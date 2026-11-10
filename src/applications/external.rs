//! Registro privado de aplicaciones externas (HU-32, decisión 6): lo que la persona anota de
//! aplicaciones hechas fuera de la plataforma. Nadie más lo ve, no pasa por la máquina de
//! estados ni por el consentimiento. `notes` va cifrada.

use crate::offers::input::{fecha, limpiar, Ymd};

pub const MAX_RECORDS: i64 = 200;
pub const STATUSES: [&str; 6] = [
    "applied",
    "interviewing",
    "offer",
    "rejected",
    "withdrawn",
    "accepted",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalInput {
    pub company_name: String,
    pub position: String,
    /// `AAAA-MM-DD`.
    pub applied_on: String,
    pub status: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalError {
    /// 422 `campo_no_permitido`.
    UnknownField,
    /// 422 `datos_invalidos`, con texto fijo.
    Invalid(&'static str),
}

/// Valida el cuerpo. `hoy` entra como parámetro: `applied_on` va de hoy a cinco años atrás.
pub fn parse_external(cuerpo: &[u8], hoy: Ymd) -> Result<ExternalInput, ExternalError> {
    use serde_json::Value;
    const FORMATO: ExternalError =
        ExternalError::Invalid("El cuerpo no tiene el formato esperado.");
    let valor: Value = serde_json::from_slice(cuerpo).map_err(|_| FORMATO)?;
    let objeto = valor.as_object().ok_or(FORMATO)?;
    if objeto.keys().any(|k| {
        !["company_name", "position", "applied_on", "status", "notes"].contains(&k.as_str())
    }) {
        return Err(ExternalError::UnknownField);
    }
    let texto = |k: &str| objeto.get(k).and_then(Value::as_str);
    let nombre = |k: &str, msg: &'static str| -> Result<String, ExternalError> {
        let t = limpiar(texto(k).ok_or(ExternalError::Invalid(msg))?, false);
        if (2..=120).contains(&t.chars().count()) {
            Ok(t)
        } else {
            Err(ExternalError::Invalid(msg))
        }
    };
    let company_name = nombre(
        "company_name",
        "El nombre de la empresa debe tener de 2 a 120 caracteres.",
    )?;
    let position = nombre("position", "El puesto debe tener de 2 a 120 caracteres.")?;

    let fecha_texto =
        texto("applied_on").ok_or(ExternalError::Invalid("La fecha no es válida."))?;
    let f = fecha(fecha_texto).ok_or(ExternalError::Invalid("La fecha no es válida."))?;
    let (y, m, d) = hoy;
    let hace_cinco =
        if m == 2 && d == 29 && !((y - 5) % 4 == 0 && ((y - 5) % 100 != 0 || (y - 5) % 400 == 0)) {
            (y - 5, 2, 28)
        } else {
            (y - 5, m, d)
        };
    if f > hoy || f < hace_cinco {
        return Err(ExternalError::Invalid(
            "La fecha debe ser de hoy a cinco años atrás.",
        ));
    }

    let status = texto("status")
        .filter(|s| STATUSES.contains(s))
        .ok_or(ExternalError::Invalid("El estado no es válido."))?;

    let notes = match objeto.get("notes") {
        None | Some(Value::Null) => None,
        Some(Value::String(n)) => {
            let n = limpiar(n, true);
            if n.chars().count() > 2000 {
                return Err(ExternalError::Invalid(
                    "Las notas admiten hasta 2000 caracteres.",
                ));
            }
            (!n.is_empty()).then_some(n)
        }
        Some(_) => return Err(ExternalError::Invalid("Las notas deben ser texto.")),
    };
    Ok(ExternalInput {
        company_name,
        position,
        applied_on: fecha_texto.to_string(),
        status: status.to_string(),
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const HOY: Ymd = (2026, 11, 10);

    fn ok() -> Value {
        json!({"company_name": "Banco X", "position": "Analista",
               "applied_on": "2026-11-05", "status": "interviewing",
               "notes": "Hablé con RRHH, esperan al 20."})
    }

    fn p(v: Value) -> Result<ExternalInput, ExternalError> {
        parse_external(v.to_string().as_bytes(), HOY)
    }

    fn invalido(v: Value) {
        let r = p(v.clone());
        assert!(matches!(r, Err(ExternalError::Invalid(_))), "{v}: {r:?}");
    }

    #[test]
    fn un_cuerpo_valido() {
        let r = p(ok()).unwrap();
        assert_eq!(r.company_name, "Banco X");
        assert_eq!(r.position, "Analista");
        assert_eq!(r.applied_on, "2026-11-05");
        assert_eq!(r.status, "interviewing");
        assert_eq!(r.notes.as_deref(), Some("Hablé con RRHH, esperan al 20."));
    }

    #[test]
    fn las_notas_son_opcionales_y_vacias_es_ausente() {
        let mut c = ok();
        c.as_object_mut().unwrap().remove("notes");
        assert_eq!(p(c.clone()).unwrap().notes, None);
        c["notes"] = Value::Null;
        assert_eq!(p(c.clone()).unwrap().notes, None);
        c["notes"] = json!("   ");
        assert_eq!(p(c).unwrap().notes, None);
    }

    #[test]
    fn se_recortan_los_textos() {
        let mut c = ok();
        c["company_name"] = json!("  Banco X  ");
        c["position"] = json!("\tAnalista\n");
        let r = p(c).unwrap();
        assert_eq!(r.company_name, "Banco X");
        assert_eq!(r.position, "Analista");
    }

    #[test]
    fn longitudes() {
        for campo in ["company_name", "position"] {
            let mut c = ok();
            c[campo] = json!("a");
            invalido(c.clone());
            c[campo] = json!("ab");
            assert!(p(c.clone()).is_ok());
            c[campo] = json!("a".repeat(120));
            assert!(p(c.clone()).is_ok());
            c[campo] = json!("a".repeat(121));
            invalido(c.clone());
            c[campo] = json!("   ");
            invalido(c);
        }
        let mut c = ok();
        c["notes"] = json!("n".repeat(2000));
        assert!(p(c.clone()).is_ok());
        c["notes"] = json!("n".repeat(2001));
        invalido(c);
        // En caracteres, no en bytes.
        let mut c = ok();
        c["company_name"] = json!("é".repeat(120));
        assert!(p(c).is_ok());
    }

    #[test]
    fn la_fecha_va_de_hoy_a_cinco_anios_atras() {
        let con = |f: &str| {
            let mut c = ok();
            c["applied_on"] = json!(f);
            p(c)
        };
        assert!(con("2026-11-10").is_ok());
        assert!(con("2021-11-10").is_ok());
        assert!(matches!(con("2026-11-11"), Err(ExternalError::Invalid(_))));
        assert!(matches!(con("2021-11-09"), Err(ExternalError::Invalid(_))));
        for mala in [
            "2026-13-01",
            "2026-02-30",
            "05/11/2026",
            "2026-1-5",
            "",
            "hoy",
        ] {
            assert!(
                matches!(con(mala), Err(ExternalError::Invalid(_))),
                "{mala}"
            );
        }
    }

    #[test]
    fn el_estado_es_uno_de_seis() {
        for s in STATUSES {
            let mut c = ok();
            c["status"] = json!(s);
            assert!(p(c).is_ok(), "{s}");
        }
        for malo in ["", "Applied", "hired", "shortlisted", "interview"] {
            let mut c = ok();
            c["status"] = json!(malo);
            invalido(c);
        }
    }

    #[test]
    fn faltan_obligatorios_o_mal_tipados() {
        for campo in ["company_name", "position", "applied_on", "status"] {
            let mut c = ok();
            c.as_object_mut().unwrap().remove(campo);
            invalido(c.clone());
            c[campo] = Value::Null;
            invalido(c.clone());
            c[campo] = json!(5);
            invalido(c);
        }
        let mut c = ok();
        c["notes"] = json!(5);
        invalido(c);
    }

    #[test]
    fn campos_de_mas_son_campo_no_permitido() {
        for extra in ["id", "created_at", "applicant_id", "offer_id", "company_id"] {
            let mut c = ok();
            c[extra] = json!("x");
            assert_eq!(p(c), Err(ExternalError::UnknownField), "{extra}");
        }
    }

    #[test]
    fn no_es_un_objeto() {
        for malo in [&b""[..], b"[]", b"null", b"nope"] {
            assert!(matches!(
                parse_external(malo, HOY),
                Err(ExternalError::Invalid(_))
            ));
        }
    }
}

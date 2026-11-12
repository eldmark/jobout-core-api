//! Validación pura del cuerpo de una oferta (`OfferInput`) y las reglas que no tocan la base:
//! qué falta para publicar y qué campos congela una aplicación.
//!
//! El texto de una oferta es **no confiable**: nunca se interpreta como HTML ni se «limpia» de
//! etiquetas. Solo se recorta, se le quitan los caracteres de control y de dirección (que sirven
//! para esconder o invertir texto) y se miden sus longitudes en caracteres Unicode. Los mensajes de
//! error son textos fijos por causa: jamás repiten lo recibido.

use std::collections::HashSet;

use serde_json::{Map, Value};

pub const MAX_SKILLS: usize = 30;
pub const MAX_LANGUAGES: usize = 5;
pub const MAX_LICENSES: usize = 5;

const CAMPOS: [&str; 22] = [
    "title",
    "description",
    "is_confidential",
    "is_experience_required",
    "required_experience",
    "main_language_id",
    "schedule",
    "required_availability_to_travel",
    "vacancies",
    "hiring_date",
    "profession_id",
    "employment_sector_id",
    "job_hierarchy_id",
    "minimum_academic_level_id",
    "state_id",
    "city_id",
    "salary_range_id",
    "working_shift_id",
    "contract_type_id",
    "skills",
    "languages",
    "driver_license_ids",
];
const CAMPOS_SKILL: [&str; 3] = ["skill_id", "is_required", "min_level"];
const CAMPOS_IDIOMA: [&str; 2] = ["language_id", "min_proficiency"];
const NIVELES: [&str; 4] = ["beginner", "intermediate", "advanced", "expert"];
const DOMINIOS: [&str; 4] = ["native", "advanced", "intermediate", "basic"];

/// Por qué se rechaza un cuerpo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    /// Un campo que el contrato no conoce → 422 `campo_no_permitido`.
    UnknownField,
    /// Un valor inválido → 422 `datos_invalidos`, con un texto fijo.
    Invalid(&'static str),
}

use InputError::{Invalid, UnknownField};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillReq {
    pub skill_id: String,
    pub is_required: bool,
    pub min_level: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageReq {
    pub language_id: String,
    pub min_proficiency: String,
}

/// Una oferta ya validada (y, al leerla de la base, la misma forma): los ids son UUID en
/// minúsculas, los requisitos van ordenados por id para que dos ofertas iguales sean iguales.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Offer {
    pub title: String,
    pub description: Option<String>,
    pub is_confidential: bool,
    pub is_experience_required: bool,
    pub required_experience: Option<i32>,
    pub main_language_id: Option<String>,
    pub schedule: Option<String>,
    pub required_availability_to_travel: bool,
    pub vacancies: Option<i32>,
    pub hiring_date: Option<String>,
    pub profession_id: Option<String>,
    pub employment_sector_id: Option<String>,
    pub job_hierarchy_id: Option<String>,
    pub minimum_academic_level_id: Option<String>,
    pub state_id: Option<String>,
    pub city_id: Option<String>,
    pub salary_range_id: Option<String>,
    pub working_shift_id: Option<String>,
    pub contract_type_id: Option<String>,
    pub skills: Vec<SkillReq>,
    pub languages: Vec<LanguageReq>,
    pub driver_license_ids: Vec<String>,
}

impl Offer {
    pub fn has_requirements(&self) -> bool {
        !self.skills.is_empty() || !self.languages.is_empty() || !self.driver_license_ids.is_empty()
    }
}

// ------------------------------------------------------------------ texto

/// Caracteres de formato invisibles o de dirección: esconden o invierten texto.
fn es_invisible(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}'
        | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
}

/// Quita controles y caracteres invisibles. Con `multilinea`, el salto de línea y el tabulador se
/// conservan; el retorno de carro nunca. Después recorta.
pub fn limpiar(s: &str, multilinea: bool) -> String {
    let limpio: String = s
        .chars()
        .filter(|&c| {
            if es_invisible(c) {
                return false;
            }
            if multilinea && (c == '\n' || c == '\t') {
                return true;
            }
            !c.is_control()
        })
        .collect();
    limpio.trim().to_string()
}

// ------------------------------------------------------------------ fechas

pub type Ymd = (i32, u32, u32);

/// `AAAA-MM-DD` real (con años bisiestos).
pub fn fecha(s: &str) -> Option<Ymd> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = &s[r];
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    let (y, m, d) = (num(0..4)? as i32, num(5..7)?, num(8..10)?);
    let dias = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if bisiesto(y) => 29,
        2 => 28,
        _ => return None,
    };
    (y >= 1900 && (1..=dias).contains(&d)).then_some((y, m, d))
}

fn bisiesto(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// La misma fecha dentro de dos años (el 29 de febrero cae en el 28 si el año no es bisiesto).
pub fn mas_dos_anios((y, m, d): Ymd) -> Ymd {
    let y2 = y + 2;
    if m == 2 && d == 29 && !bisiesto(y2) {
        (y2, 2, 28)
    } else {
        (y2, m, d)
    }
}

/// `hiring_date`: hoy o futura, y a lo sumo dentro de 2 años.
pub fn rango_de_contratacion(fecha_texto: &str, hoy: Ymd) -> Result<(), InputError> {
    let f = fecha(fecha_texto).ok_or(Invalid("La fecha de contratación no es válida."))?;
    if f < hoy || f > mas_dos_anios(hoy) {
        return Err(Invalid(
            "La fecha de contratación debe ser de hoy a dos años.",
        ));
    }
    Ok(())
}

// ------------------------------------------------------------------ parseo

/// UUID en texto canónico (8-4-4-4-12 hexadecimal) → minúsculas.
pub fn uuid_canonico(s: &str) -> Option<String> {
    let partes: Vec<&str> = s.split('-').collect();
    let largos = [8, 4, 4, 4, 12];
    let ok = partes.len() == 5
        && partes
            .iter()
            .zip(largos)
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()));
    ok.then(|| s.to_ascii_lowercase())
}

fn invalido_id() -> InputError {
    Invalid("Un dato del catálogo no es válido.")
}

fn objeto<'a>(v: &'a Value, permitidos: &[&str]) -> Result<&'a Map<String, Value>, InputError> {
    let m = v
        .as_object()
        .ok_or(Invalid("El cuerpo no tiene el formato esperado."))?;
    if m.keys().any(|k| !permitidos.contains(&k.as_str())) {
        return Err(UnknownField);
    }
    Ok(m)
}

fn texto_opcional<'a>(m: &'a Map<String, Value>, k: &str) -> Result<Option<&'a str>, InputError> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(Invalid("Un dato no tiene el formato esperado.")),
    }
}

fn booleano(m: &Map<String, Value>, k: &str, defecto: bool) -> Result<bool, InputError> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(defecto),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(Invalid("Un dato no tiene el formato esperado.")),
    }
}

fn entero(m: &Map<String, Value>, k: &str) -> Result<Option<i64>, InputError> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .map(Some)
            .ok_or(Invalid("Un número no tiene el formato esperado.")),
    }
}

fn id_opcional(m: &Map<String, Value>, k: &str) -> Result<Option<String>, InputError> {
    texto_opcional(m, k)?
        .map(|s| uuid_canonico(s).ok_or_else(invalido_id))
        .transpose()
}

/// Texto de una línea o de varias; vacío = ausente; `max` en caracteres.
fn texto_limpio(
    m: &Map<String, Value>,
    k: &str,
    multilinea: bool,
    min: usize,
    max: usize,
    mensaje: &'static str,
) -> Result<Option<String>, InputError> {
    let Some(crudo) = texto_opcional(m, k)? else {
        return Ok(None);
    };
    let s = limpiar(crudo, multilinea);
    if s.is_empty() {
        return Ok(None);
    }
    let n = s.chars().count();
    if n < min || n > max {
        return Err(Invalid(mensaje));
    }
    Ok(Some(s))
}

fn lista<'a>(m: &'a Map<String, Value>, k: &str, max: usize) -> Result<&'a [Value], InputError> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(a)) if a.len() <= max => Ok(a),
        Some(Value::Array(_)) => Err(Invalid("Hay demasiados elementos en una lista.")),
        Some(_) => Err(Invalid("Una lista no tiene el formato esperado.")),
    }
}

/// Valida el cuerpo de `POST`/`PUT` de una oferta. No mira la base (catálogos) ni el rango de
/// `hiring_date` (ver [`rango_de_contratacion`]): eso lo hace quien llama.
pub fn parse_offer(cuerpo: &[u8]) -> Result<Offer, InputError> {
    let valor: Value = serde_json::from_slice(cuerpo)
        .map_err(|_| Invalid("El cuerpo no tiene el formato esperado."))?;
    let m = objeto(&valor, &CAMPOS)?;

    let title = texto_limpio(
        m,
        "title",
        false,
        3,
        120,
        "El título debe tener entre 3 y 120 caracteres.",
    )?
    .ok_or(Invalid("El título debe tener entre 3 y 120 caracteres."))?;
    let description = texto_limpio(
        m,
        "description",
        true,
        20,
        5000,
        "La descripción debe tener entre 20 y 5000 caracteres.",
    )?;
    let schedule = texto_limpio(
        m,
        "schedule",
        false,
        0,
        120,
        "El horario admite hasta 120 caracteres.",
    )?;

    let is_experience_required = booleano(m, "is_experience_required", false)?;
    let required_experience = match entero(m, "required_experience")? {
        None => None,
        Some(n) if (0..=50).contains(&n) => Some(n as i32),
        Some(_) => return Err(Invalid("Los años de experiencia van de 0 a 50.")),
    };
    match (is_experience_required, required_experience) {
        (true, None) => return Err(Invalid("Si se exige experiencia hay que indicar los años.")),
        (false, Some(_)) => {
            return Err(Invalid(
                "Los años de experiencia solo van si se exige experiencia.",
            ))
        }
        _ => {}
    }
    let vacancies = match entero(m, "vacancies")? {
        None => None,
        Some(n) if (1..=999).contains(&n) => Some(n as i32),
        Some(_) => return Err(Invalid("Las vacantes van de 1 a 999.")),
    };
    let hiring_date = match texto_opcional(m, "hiring_date")? {
        None => None,
        Some(f) => {
            fecha(f).ok_or(Invalid("La fecha de contratación no es válida."))?;
            Some(f.to_string())
        }
    };

    let mut skills = Vec::new();
    let mut vistos = HashSet::new();
    for item in lista(m, "skills", MAX_SKILLS)? {
        let o = objeto(item, &CAMPOS_SKILL)?;
        let skill_id = id_opcional(o, "skill_id")?.ok_or(Invalid("Una skill no es válida."))?;
        if !vistos.insert(skill_id.clone()) {
            return Err(Invalid("Hay una skill repetida."));
        }
        let min_level = texto_opcional(o, "min_level")?
            .map(|n| {
                NIVELES
                    .contains(&n)
                    .then(|| n.to_string())
                    .ok_or(Invalid("Un nivel de skill no es válido."))
            })
            .transpose()?;
        skills.push(SkillReq {
            skill_id,
            is_required: booleano(o, "is_required", true)?,
            min_level,
        });
    }
    skills.sort_by(|a, b| a.skill_id.cmp(&b.skill_id));

    let mut languages = Vec::new();
    let mut vistos = HashSet::new();
    for item in lista(m, "languages", MAX_LANGUAGES)? {
        let o = objeto(item, &CAMPOS_IDIOMA)?;
        let language_id =
            id_opcional(o, "language_id")?.ok_or(Invalid("Un idioma no es válido."))?;
        if !vistos.insert(language_id.clone()) {
            return Err(Invalid("Hay un idioma repetido."));
        }
        let nivel = texto_opcional(o, "min_proficiency")?
            .filter(|n| DOMINIOS.contains(n))
            .ok_or(Invalid("Un nivel de idioma no es válido."))?;
        languages.push(LanguageReq {
            language_id,
            min_proficiency: nivel.to_string(),
        });
    }
    languages.sort_by(|a, b| a.language_id.cmp(&b.language_id));

    let mut driver_license_ids = Vec::new();
    let mut vistos = HashSet::new();
    for item in lista(m, "driver_license_ids", MAX_LICENSES)? {
        let id = item
            .as_str()
            .and_then(uuid_canonico)
            .ok_or(Invalid("Una licencia no es válida."))?;
        if !vistos.insert(id.clone()) {
            return Err(Invalid("Hay una licencia repetida."));
        }
        driver_license_ids.push(id);
    }
    driver_license_ids.sort();

    Ok(Offer {
        title,
        description,
        is_confidential: booleano(m, "is_confidential", false)?,
        is_experience_required,
        required_experience,
        main_language_id: id_opcional(m, "main_language_id")?,
        schedule,
        required_availability_to_travel: booleano(m, "required_availability_to_travel", false)?,
        vacancies,
        hiring_date,
        profession_id: id_opcional(m, "profession_id")?,
        employment_sector_id: id_opcional(m, "employment_sector_id")?,
        job_hierarchy_id: id_opcional(m, "job_hierarchy_id")?,
        minimum_academic_level_id: id_opcional(m, "minimum_academic_level_id")?,
        state_id: id_opcional(m, "state_id")?,
        city_id: id_opcional(m, "city_id")?,
        salary_range_id: id_opcional(m, "salary_range_id")?,
        working_shift_id: id_opcional(m, "working_shift_id")?,
        contract_type_id: id_opcional(m, "contract_type_id")?,
        skills,
        languages,
        driver_license_ids,
    })
}

// ------------------------------------------------------------------ reglas

/// Lo que falta para publicar (OF5), en el orden del contrato. `requirements` agrupa skill,
/// idioma y licencia. Una `hiring_date` ya pasada también cuenta.
pub fn missing_for_publish(o: &Offer, hoy: Ymd) -> Vec<&'static str> {
    let mut faltan = Vec::new();
    if o.title.trim().is_empty() {
        faltan.push("title");
    }
    if o.description.is_none() {
        faltan.push("description");
    }
    if o.vacancies.is_none() {
        faltan.push("vacancies");
    }
    if o.profession_id.is_none() {
        faltan.push("profession_id");
    }
    if o.state_id.is_none() {
        faltan.push("state_id");
    }
    if o.city_id.is_none() {
        faltan.push("city_id");
    }
    if !o.has_requirements() {
        faltan.push("requirements");
    }
    if o.hiring_date
        .as_deref()
        .and_then(fecha)
        .is_some_and(|f| f < hoy)
    {
        faltan.push("hiring_date");
    }
    faltan
}

/// ¿Cambia el cuerpo algo que una aplicación congela (OF7)? Solo `description`, `schedule`,
/// `vacancies` y `hiring_date` se pueden editar con aplicaciones.
pub fn frozen_changed(actual: &Offer, nuevo: &Offer) -> bool {
    let mut a = actual.clone();
    a.description = nuevo.description.clone();
    a.schedule = nuevo.schedule.clone();
    a.vacancies = nuevo.vacancies;
    a.hiring_date = nuevo.hiring_date.clone();
    &a != nuevo
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const U1: &str = "019a0000-0000-7000-8000-000000000001";
    const U2: &str = "019a0000-0000-7000-8000-000000000002";

    fn parse(v: Value) -> Result<Offer, InputError> {
        parse_offer(v.to_string().as_bytes())
    }

    fn completa() -> Offer {
        Offer {
            title: "Desarrollador".into(),
            description: Some("Una descripción suficientemente larga.".into()),
            vacancies: Some(2),
            profession_id: Some(U1.into()),
            state_id: Some(U1.into()),
            city_id: Some(U2.into()),
            skills: vec![SkillReq {
                skill_id: U1.into(),
                is_required: true,
                min_level: None,
            }],
            ..Offer::default()
        }
    }

    #[test]
    fn limpiar_quita_controles_e_invisibles_y_recorta() {
        assert_eq!(limpiar("  hola\u{0}\u{7}  ", false), "hola");
        assert_eq!(limpiar("a\nb\tc\r", false), "abc");
        assert_eq!(limpiar("a\nb\tc\r", true), "a\nb\tc");
        assert_eq!(limpiar("a\u{202E}b\u{200B}c\u{FEFF}", true), "abc");
        // El HTML no se toca.
        assert_eq!(
            limpiar("<script>alert(1)</script>", false),
            "<script>alert(1)</script>"
        );
        // Acentos, ñ y emoji se conservan.
        assert_eq!(limpiar("Año ñandú 🚀", false), "Año ñandú 🚀");
    }

    #[test]
    fn las_longitudes_se_miden_en_caracteres_no_bytes() {
        let t = "á".repeat(120);
        assert!(parse(json!({"title": t})).is_ok());
        assert!(parse(json!({"title": "á".repeat(121)})).is_err());
        assert!(parse(json!({"title": "ab"})).is_err());
        assert!(parse(json!({"title": "   ab   "})).is_err());
    }

    #[test]
    fn un_campo_desconocido_es_campo_no_permitido_y_gana_a_un_valor_invalido() {
        assert_eq!(parse(json!({"title": "Abc", "age": 30})), Err(UnknownField));
        assert_eq!(parse(json!({"title": 5, "gender": "f"})), Err(UnknownField));
        assert_eq!(
            parse(json!({"title": "Abc", "skills": [{"skill_id": U1, "x": 1}]})),
            Err(UnknownField)
        );
        assert_eq!(
            parse(
                json!({"title": "Abc", "languages": [{"language_id": U1, "min_proficiency": "basic", "age": 1}]})
            ),
            Err(UnknownField)
        );
    }

    #[test]
    fn cuerpos_que_no_son_un_objeto_son_invalidos() {
        for b in ["[]", "5", "\"x\"", "null", "", "{"] {
            assert!(matches!(parse_offer(b.as_bytes()), Err(Invalid(_))), "{b}");
        }
    }

    #[test]
    fn los_mensajes_no_repiten_lo_recibido() {
        let secreto = "secreto-xyz-77";
        for cuerpo in [
            json!({"title": secreto, "vacancies": 0}),
            json!({"title": secreto, "profession_id": secreto}),
            json!({"title": secreto, "skills": [{"skill_id": secreto}]}),
            json!({"title": secreto, "hiring_date": secreto}),
            json!({"title": secreto, "description": secreto}),
        ] {
            if let Err(Invalid(m)) = parse(cuerpo) {
                assert!(!m.contains(secreto));
            } else {
                panic!("debía fallar");
            }
        }
    }

    #[test]
    fn la_experiencia_va_con_su_bandera() {
        assert!(parse(json!({"title": "Abc", "is_experience_required": true})).is_err());
        assert!(parse(
            json!({"title": "Abc", "is_experience_required": true, "required_experience": 3})
        )
        .is_ok());
        assert!(parse(json!({"title": "Abc", "required_experience": 3})).is_err());
        assert!(parse(
            json!({"title": "Abc", "is_experience_required": true, "required_experience": 51})
        )
        .is_err());
        assert!(parse(
            json!({"title": "Abc", "is_experience_required": true, "required_experience": 2.5})
        )
        .is_err());
    }

    #[test]
    fn los_requisitos_se_ordenan_y_no_se_repiten() {
        let o = parse(json!({"title": "Abc",
            "skills": [{"skill_id": U2}, {"skill_id": U1, "is_required": false}],
            "driver_license_ids": [U2, U1]}))
        .unwrap();
        assert_eq!(o.skills[0].skill_id, U1);
        assert!(!o.skills[0].is_required);
        assert!(
            o.skills[1].is_required,
            "por defecto una skill es requerida"
        );
        assert_eq!(o.driver_license_ids, [U1, U2]);
        assert!(parse(json!({"title": "Abc", "driver_license_ids": [U1, U1]})).is_err());
        assert!(parse(
            json!({"title": "Abc", "skills": [{"skill_id": U1}, {"skill_id": U1.to_uppercase()}]})
        )
        .is_err());
    }

    #[test]
    fn los_ids_se_normalizan_a_minusculas() {
        let o = parse(json!({"title": "Abc", "profession_id": U1.to_uppercase()})).unwrap();
        assert_eq!(o.profession_id.as_deref(), Some(U1));
        assert!(parse(json!({"title": "Abc", "profession_id": "x"})).is_err());
        assert!(parse(json!({"title": "Abc", "profession_id": 7})).is_err());
    }

    #[test]
    fn fechas_reales_y_bisiestas() {
        assert_eq!(fecha("2028-02-29"), Some((2028, 2, 29)));
        assert_eq!(fecha("2027-02-29"), None);
        assert_eq!(fecha("2027-13-01"), None);
        assert_eq!(fecha("2027-1-01"), None);
        assert_eq!(fecha("27-01-01"), None);
        assert_eq!(fecha("2027-01-01 "), None);
        assert_eq!(mas_dos_anios((2026, 11, 10)), (2028, 11, 10));
        assert_eq!(mas_dos_anios((2026, 2, 28)), (2028, 2, 28));
        assert_eq!(mas_dos_anios((2028, 2, 29)), (2030, 2, 28));
    }

    #[test]
    fn el_rango_de_contratacion_es_de_hoy_a_dos_anios_sin_reloj_de_pared() {
        let hoy = (2026, 11, 10);
        assert!(rango_de_contratacion("2026-11-10", hoy).is_ok());
        assert!(rango_de_contratacion("2026-11-09", hoy).is_err());
        assert!(rango_de_contratacion("2028-11-10", hoy).is_ok());
        assert!(rango_de_contratacion("2028-11-11", hoy).is_err());
        assert!(rango_de_contratacion("2026-02-30", hoy).is_err());
    }

    #[test]
    fn publicar_pide_lo_minimo_en_el_orden_del_contrato() {
        let hoy = (2026, 11, 10);
        assert!(missing_for_publish(&completa(), hoy).is_empty());
        let vacia = Offer {
            title: "Solo título".into(),
            ..Offer::default()
        };
        assert_eq!(
            missing_for_publish(&vacia, hoy),
            [
                "description",
                "vacancies",
                "profession_id",
                "state_id",
                "city_id",
                "requirements"
            ]
        );
        // Cualquiera de los tres requisitos cuenta.
        for f in [
            |o: &mut Offer| o.skills.clear(),
            |o: &mut Offer| o.driver_license_ids = vec![U1.into()],
            |o: &mut Offer| {
                o.languages = vec![LanguageReq {
                    language_id: U1.into(),
                    min_proficiency: "basic".into(),
                }]
            },
        ] {
            let mut o = completa();
            f(&mut o);
            let esperado = if o.has_requirements() { 0 } else { 1 };
            assert_eq!(missing_for_publish(&o, hoy).len(), esperado);
        }
        let mut pasada = completa();
        pasada.hiring_date = Some("2026-11-09".into());
        assert_eq!(missing_for_publish(&pasada, hoy), ["hiring_date"]);
        pasada.hiring_date = Some("2026-11-10".into());
        assert!(missing_for_publish(&pasada, hoy).is_empty());
    }

    #[test]
    fn solo_cuatro_campos_son_editables_con_aplicaciones() {
        let base = completa();
        let mut editable = base.clone();
        editable.description = Some("Otra descripción distinta y larga.".into());
        editable.schedule = Some("Noches".into());
        editable.vacancies = Some(9);
        editable.hiring_date = Some("2027-01-01".into());
        assert!(!frozen_changed(&base, &editable));
        assert!(!frozen_changed(&base, &base));

        type Cambio = Box<dyn Fn(&mut Offer)>;
        let cambios: Vec<Cambio> = vec![
            Box::new(|o| o.title = "Otro".into()),
            Box::new(|o| o.is_confidential = true),
            Box::new(|o| o.is_experience_required = true),
            Box::new(|o| o.required_experience = Some(1)),
            Box::new(|o| o.main_language_id = Some(U1.into())),
            Box::new(|o| o.required_availability_to_travel = true),
            Box::new(|o| o.profession_id = Some(U2.into())),
            Box::new(|o| o.employment_sector_id = Some(U2.into())),
            Box::new(|o| o.job_hierarchy_id = Some(U2.into())),
            Box::new(|o| o.minimum_academic_level_id = Some(U2.into())),
            Box::new(|o| o.state_id = Some(U2.into())),
            Box::new(|o| o.city_id = Some(U1.into())),
            Box::new(|o| o.salary_range_id = Some(U2.into())),
            Box::new(|o| o.working_shift_id = Some(U2.into())),
            Box::new(|o| o.contract_type_id = Some(U2.into())),
            Box::new(|o| o.skills.clear()),
            Box::new(|o| o.skills[0].is_required = false),
            Box::new(|o| o.skills[0].min_level = Some("expert".into())),
            Box::new(|o| {
                o.languages.push(LanguageReq {
                    language_id: U1.into(),
                    min_proficiency: "basic".into(),
                })
            }),
            Box::new(|o| o.driver_license_ids.push(U1.into())),
        ];
        for (i, c) in cambios.iter().enumerate() {
            let mut n = base.clone();
            c(&mut n);
            assert!(frozen_changed(&base, &n), "el cambio {i} no se detectó");
        }
    }
}

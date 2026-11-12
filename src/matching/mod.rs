//! Matching entre una persona y una oferta (Fase 5b, paso 8): una función **pura** que da un
//! puntaje 0–100 y su desglose. Contrato: `api.md`, «Matching (Fase 5b)».
//!
//! Lo único que entra es habilidades, idiomas, periodos de experiencia y el `sort_order` del nivel
//! académico de la persona. No hay ninguna otra columna de la persona al alcance de este módulo
//! (edad, género, estado civil, nombre, foto, dirección): una prueba lo vigila (MA4, MA5). El
//! reloj («hoy») entra como parámetro; nada lee la hora.

pub mod store;

use serde_json::{json, Value};

use crate::offers::input::fecha;

/// `(año, mes, día)`, como en `offers::input`.
pub type Ymd = (i32, u32, u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Beginner,
    Intermediate,
    Advanced,
    Expert,
}

impl Level {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "beginner" => Self::Beginner,
            "intermediate" => Self::Intermediate,
            "advanced" => Self::Advanced,
            "expert" => Self::Expert,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Beginner => "beginner",
            Self::Intermediate => "intermediate",
            Self::Advanced => "advanced",
            Self::Expert => "expert",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Proficiency {
    Basic,
    Intermediate,
    Advanced,
    Native,
}

impl Proficiency {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "basic" => Self::Basic,
            "intermediate" => Self::Intermediate,
            "advanced" => Self::Advanced,
            "native" => Self::Native,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Intermediate => "intermediate",
            Self::Advanced => "advanced",
            Self::Native => "native",
        }
    }
}

/// Un elemento de catálogo tal como sale en el desglose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cat {
    pub id: String,
    pub code: String,
    pub name: String,
}

impl Cat {
    fn json(&self) -> Value {
        json!({"id": self.id, "code": self.code, "name": self.name})
    }
}

/// Un dato de la persona que puede no estar a la vista (la persona no lo compartió con el
/// reclutador). Un componente que depende de un dato oculto **no aplica**: nunca se supone vacío.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Data<T> {
    Known(T),
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Period {
    pub start: Ymd,
    /// `None`: trabajo actual, cuenta hasta hoy.
    pub end: Option<Ymd>,
}

/// Lo único que el matching sabe de una persona.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// `(id de la habilidad, nivel)`.
    pub skills: Data<Vec<(String, Option<Level>)>>,
    /// `(id del idioma, dominio)`.
    pub languages: Data<Vec<(String, Proficiency)>>,
    pub experience: Data<Vec<Period>>,
    /// `sort_order` de su último nivel académico; `None` si no lo declaró.
    pub academic_sort: Data<Option<i32>>,
}

impl Profile {
    pub fn empty() -> Self {
        Self {
            skills: Data::Known(vec![]),
            languages: Data::Known(vec![]),
            experience: Data::Known(vec![]),
            academic_sort: Data::Known(None),
        }
    }

    /// Sin habilidades, idiomas, experiencia ni nivel académico (todo a la vista y vacío).
    pub fn is_empty(&self) -> bool {
        self.skills == Data::Known(vec![])
            && self.languages == Data::Known(vec![])
            && self.experience == Data::Known(vec![])
            && self.academic_sort == Data::Known(None)
    }

    /// Arma el perfil desde lo que la persona **compartió** (`applications::shared_view`): una
    /// clave ausente es un dato oculto. Cualquier otra clave de `view` (nombre, contacto, edad,
    /// género…) se ignora. `sort_of` da el `sort_order` de un nivel académico por su id.
    pub fn from_shared(view: &Value, sort_of: impl Fn(&str) -> Option<i32>) -> Self {
        let lista = |k: &str| view.get(k).and_then(Value::as_array);
        let id_de = |v: &Value, k: &str| -> Option<String> {
            v.get(k)?.get("id")?.as_str().map(str::to_string)
        };
        let skills = match lista("skills") {
            Some(a) => Data::Known(
                a.iter()
                    .filter_map(|e| {
                        let nivel = e
                            .get("level")
                            .and_then(Value::as_str)
                            .and_then(Level::parse);
                        Some((id_de(e, "skill")?, nivel))
                    })
                    .collect(),
            ),
            None => Data::Hidden,
        };
        let languages = match lista("languages") {
            Some(a) => Data::Known(
                a.iter()
                    .filter_map(|e| {
                        let d = Proficiency::parse(e.get("proficiency")?.as_str()?)?;
                        Some((id_de(e, "language")?, d))
                    })
                    .collect(),
            ),
            None => Data::Hidden,
        };
        let experience = match lista("experience") {
            Some(a) => Data::Known(
                a.iter()
                    .filter_map(|e| {
                        let start = fecha(e.get("start_date")?.as_str()?)?;
                        let end = match e.get("end_date") {
                            None | Some(Value::Null) => None,
                            Some(x) => Some(fecha(x.as_str()?)?),
                        };
                        Some(Period { start, end })
                    })
                    .collect(),
            ),
            None => Data::Hidden,
        };
        let academic_sort = match view.get("education").filter(|e| e.is_object()) {
            Some(e) => Data::Known(id_de(e, "academic_level").and_then(|id| sort_of(&id))),
            None => Data::Hidden,
        };
        Self {
            skills,
            languages,
            experience,
            academic_sort,
        }
    }
}

/// Lo que una oferta pide, ya reducido a lo comparable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Requirements {
    pub skills: Vec<ReqSkill>,
    pub languages: Vec<ReqLanguage>,
    /// Solo si la oferta exige experiencia **y** pide más de 0 years_f.
    pub required_years: Option<u32>,
    /// `sort_order` del nivel académico mínimo.
    pub academic_min: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReqSkill {
    pub skill: Cat,
    pub required: bool,
    pub min_level: Option<Level>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReqLanguage {
    pub language: Cat,
    pub min_proficiency: Option<Proficiency>,
}

/// Una pieza del desglose.
#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    pub key: &'static str,
    pub weight: u32,
    pub points: f64,
    pub applies: bool,
    /// `no_compartido` si el dato no está a la vista; `no_pedido` si la oferta no lo pide.
    pub reason: Option<&'static str>,
    pub matched: Vec<Value>,
    pub partial: Vec<Value>,
    pub missing: Vec<Value>,
    pub required_years: Option<u32>,
    pub years: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub score: u8,
    /// `sin_requisitos_comparables` si ningún componente aplica.
    pub reason: Option<&'static str>,
    pub empty_profile: bool,
    pub components: Vec<Component>,
}

impl Match {
    /// `{score, breakdown}`: la forma del contrato.
    pub fn to_json(&self) -> Value {
        json!({"score": self.score, "breakdown": {
            "reason": self.reason, "empty_profile": self.empty_profile,
            "components": self.components.iter().map(Component::to_json).collect::<Vec<_>>()}})
    }
}

impl Component {
    fn to_json(&self) -> Value {
        let mut v = json!({"key": self.key, "weight": self.weight, "points": r1(self.points),
                           "applies": self.applies});
        if let Some(r) = self.reason {
            v["reason"] = json!(r);
        }
        if self.applies && (self.key == "skills" || self.key == "languages") {
            v["matched"] = json!(self.matched);
            v["partial"] = json!(self.partial);
            v["missing"] = json!(self.missing);
        }
        if self.applies && self.key == "experience" {
            v["required_years"] = json!(self.required_years);
            v["years"] = json!(self.years.map(r1));
        }
        v
    }
}

fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

const W_SKILLS: u32 = 50;
const W_LANGUAGES: u32 = 15;
const W_EXPERIENCE: u32 = 20;
const W_ACADEMIC: u32 = 15;

/// Días desde 1970-01-01 (calendario gregoriano proléptico).
fn dias((y, m, d): Ymd) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Años de experiencia: la **unión** de los periodos (dos empleos a la vez no suman doble), con
/// los actuales hasta hoy y sin contar lo que todavía no pasó.
fn anios(periodos: &[Period], today: Ymd) -> f64 {
    let hoy = dias(today);
    let mut v: Vec<(i64, i64)> = periodos
        .iter()
        .map(|p| (dias(p.start), p.end.map_or(hoy, dias).min(hoy)))
        .filter(|(a, b)| b > a)
        .collect();
    v.sort_unstable();
    let (mut total, mut hasta) = (0_i64, i64::MIN);
    for (a, b) in v {
        let a = a.max(hasta);
        if b > a {
            total += b - a;
        }
        hasta = hasta.max(b);
    }
    total as f64 / 365.25
}

fn aplica(key: &'static str, weight: u32, razon: &'static str) -> Component {
    Component {
        key,
        weight,
        points: 0.0,
        applies: false,
        reason: Some(razon),
        matched: vec![],
        partial: vec![],
        missing: vec![],
        required_years: None,
        years: None,
    }
}

fn habilidades(profile: &Profile, req: &Requirements) -> Component {
    if req.skills.is_empty() {
        return aplica("skills", W_SKILLS, "no_pedido");
    }
    let Data::Known(tiene) = &profile.skills else {
        return aplica("skills", W_SKILLS, "no_compartido");
    };
    let mut c = aplica("skills", W_SKILLS, "");
    c.applies = true;
    c.reason = None;
    let (mut ganado, mut total) = (0.0, 0.0);
    for r in &req.skills {
        let peso = if r.required { 2.0 } else { 1.0 };
        total += peso;
        let base = json!({"skill": r.skill.json(), "required": r.required});
        let suya = tiene.iter().find(|(id, _)| *id == r.skill.id);
        match (suya, r.min_level) {
            (None, _) => c.missing.push(base),
            (Some(_), None) => {
                ganado += peso;
                c.matched.push(base);
            }
            (Some((_, Some(n))), Some(min)) if *n >= min => {
                ganado += peso;
                c.matched.push(base);
            }
            (Some((_, nivel)), Some(min)) => {
                ganado += peso / 2.0;
                let mut p = base;
                p["min_level"] = json!(min.as_str());
                p["level"] = json!(nivel.map(Level::as_str));
                c.partial.push(p);
            }
        }
    }
    c.points = f64::from(W_SKILLS) * ganado / total;
    c
}

fn idiomas(profile: &Profile, req: &Requirements) -> Component {
    if req.languages.is_empty() {
        return aplica("languages", W_LANGUAGES, "no_pedido");
    }
    let Data::Known(tiene) = &profile.languages else {
        return aplica("languages", W_LANGUAGES, "no_compartido");
    };
    let mut c = aplica("languages", W_LANGUAGES, "");
    c.applies = true;
    c.reason = None;
    let mut ganado = 0.0;
    for r in &req.languages {
        let mut item = json!({"language": r.language.json(),
                              "min_proficiency": r.min_proficiency.map(Proficiency::as_str)});
        match tiene.iter().find(|(id, _)| *id == r.language.id) {
            None => c.missing.push(item),
            Some((_, d)) => {
                item["proficiency"] = json!(d.as_str());
                if r.min_proficiency.is_none_or(|min| *d >= min) {
                    ganado += 1.0;
                    c.matched.push(item);
                } else {
                    ganado += 0.5;
                    c.partial.push(item);
                }
            }
        }
    }
    c.points = f64::from(W_LANGUAGES) * ganado / req.languages.len() as f64;
    c
}

fn experiencia(profile: &Profile, req: &Requirements, today: Ymd) -> Component {
    let Some(pedidos) = req.required_years else {
        return aplica("experience", W_EXPERIENCE, "no_pedido");
    };
    let Data::Known(periodos) = &profile.experience else {
        return aplica("experience", W_EXPERIENCE, "no_compartido");
    };
    let years_f = anios(periodos, today);
    let mut c = aplica("experience", W_EXPERIENCE, "");
    c.applies = true;
    c.reason = None;
    c.required_years = Some(pedidos);
    c.years = Some(r1(years_f));
    c.points = f64::from(W_EXPERIENCE) * (years_f / f64::from(pedidos)).min(1.0);
    c
}

fn nivel_academico(profile: &Profile, req: &Requirements) -> Component {
    let Some(min) = req.academic_min else {
        return aplica("academic_level", W_ACADEMIC, "no_pedido");
    };
    let Data::Known(suyo) = &profile.academic_sort else {
        return aplica("academic_level", W_ACADEMIC, "no_compartido");
    };
    let mut c = aplica("academic_level", W_ACADEMIC, "");
    c.applies = true;
    c.reason = None;
    if suyo.is_some_and(|n| n >= min) {
        c.points = f64::from(W_ACADEMIC);
    }
    c
}

/// El puntaje de una persona frente a una oferta. `today` es el único reloj.
///
/// `score` = puntos de los componentes que **aplican** ÷ sus pesos × 100, redondeado. Un
/// componente que la oferta no pide, o cuyo dato la persona no compartió, no aplica y su peso no
/// entra en la cuenta.
pub fn compute(profile: &Profile, req: &Requirements, today: Ymd) -> Match {
    let components = vec![
        habilidades(profile, req),
        idiomas(profile, req),
        experiencia(profile, req, today),
        nivel_academico(profile, req),
    ];
    let pesos: u32 = components
        .iter()
        .filter(|c| c.applies)
        .map(|c| c.weight)
        .sum();
    let puntos: f64 = components
        .iter()
        .filter(|c| c.applies)
        .map(|c| c.points)
        .sum();
    let (score, reason) = if pesos == 0 {
        (0, Some("sin_requisitos_comparables"))
    } else {
        (
            (puntos / f64::from(pesos) * 100.0)
                .round()
                .clamp(0.0, 100.0) as u8,
            None,
        )
    };
    Match {
        score,
        reason,
        empty_profile: profile.is_empty(),
        components,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOY: Ymd = (2026, 11, 11);

    fn cat(n: &str) -> Cat {
        Cat {
            id: format!("id-{n}"),
            code: n.into(),
            name: n.to_uppercase(),
        }
    }
    fn rs(n: &str, required: bool, min: Option<Level>) -> ReqSkill {
        ReqSkill {
            skill: cat(n),
            required,
            min_level: min,
        }
    }
    fn rl(n: &str, min: Option<Proficiency>) -> ReqLanguage {
        ReqLanguage {
            language: cat(n),
            min_proficiency: min,
        }
    }
    fn per(a: Ymd, b: Option<Ymd>) -> Period {
        Period { start: a, end: b }
    }
    fn perfil(
        skills: &[(&str, Option<Level>)],
        langs: &[(&str, Proficiency)],
        exp: Vec<Period>,
        acad: Option<i32>,
    ) -> Profile {
        Profile {
            skills: Data::Known(
                skills
                    .iter()
                    .map(|(n, l)| (format!("id-{n}"), *l))
                    .collect(),
            ),
            languages: Data::Known(langs.iter().map(|(n, p)| (format!("id-{n}"), *p)).collect()),
            experience: Data::Known(exp),
            academic_sort: Data::Known(acad),
        }
    }
    fn comp<'a>(m: &'a Match, k: &str) -> &'a Component {
        m.components.iter().find(|c| c.key == k).unwrap()
    }

    #[test]
    fn la_oferta_solo_de_habilidades_se_puntua_sobre_el_peso_de_las_habilidades() {
        use Level::*;
        // (habilidades de la persona, lo que pide la oferta, puntaje esperado)
        let pide = vec![rs("rust", true, None), rs("sql", false, None)];
        type Tiene<'a> = Vec<(&'a str, Option<Level>)>;
        let casos: Vec<(Tiene, u8)> = vec![
            (vec![("rust", None), ("sql", None)], 100),
            (vec![("rust", Some(Expert))], 67), // 2 de 3 puntos
            (vec![("sql", None)], 33),          // 1 de 3: falta la obligatoria
            (vec![], 0),
            (vec![("otra", None)], 0),
        ];
        for (tiene, esperado) in casos {
            let m = compute(
                &perfil(&tiene, &[], vec![], None),
                &Requirements {
                    skills: pide.clone(),
                    ..Default::default()
                },
                HOY,
            );
            assert_eq!(m.score, esperado, "{tiene:?}");
        }
    }

    #[test]
    fn ma3_faltar_una_obligatoria_baja_mas_que_faltar_una_opcional() {
        let pide = vec![rs("rust", true, None), rs("sql", false, None)];
        let req = Requirements {
            skills: pide,
            ..Default::default()
        };
        let sin_obligatoria = compute(&perfil(&[("sql", None)], &[], vec![], None), &req, HOY);
        let sin_opcional = compute(&perfil(&[("rust", None)], &[], vec![], None), &req, HOY);
        assert!(sin_obligatoria.score < sin_opcional.score);
        assert_eq!((sin_obligatoria.score, sin_opcional.score), (33, 67));
    }

    #[test]
    fn una_habilidad_ausente_no_cuenta_como_coincidencia() {
        let req = Requirements {
            skills: vec![rs("rust", true, None)],
            ..Default::default()
        };
        let m = compute(&perfil(&[("otra", None)], &[], vec![], None), &req, HOY);
        let c = comp(&m, "skills");
        assert!(c.matched.is_empty() && c.partial.is_empty());
        assert_eq!(c.missing.len(), 1);
        assert_eq!(c.points, 0.0);
    }

    #[test]
    fn el_nivel_minimo_da_completo_mitad_o_nada() {
        use Level::*;
        let req = Requirements {
            skills: vec![rs("rust", true, Some(Advanced))],
            ..Default::default()
        };
        let casos = [
            (Some(Advanced), 100, "matched"),
            (Some(Expert), 100, "matched"),
            (Some(Intermediate), 50, "partial"),
            (None, 50, "partial"), // sin nivel cuando se pide uno
        ];
        for (nivel, esperado, donde) in casos {
            let m = compute(&perfil(&[("rust", nivel)], &[], vec![], None), &req, HOY);
            assert_eq!(m.score, esperado, "{nivel:?}");
            let c = comp(&m, "skills");
            let (a, b, z) = (c.matched.len(), c.partial.len(), c.missing.len());
            match donde {
                "matched" => assert_eq!((a, b, z), (1, 0, 0)),
                _ => assert_eq!((a, b, z), (0, 1, 0)),
            }
        }
        // Sin `min_level`, cualquier nivel (o ninguno) es completo.
        let libre = Requirements {
            skills: vec![rs("rust", true, None)],
            ..Default::default()
        };
        let m = compute(&perfil(&[("rust", None)], &[], vec![], None), &libre, HOY);
        assert_eq!(m.score, 100);
    }

    #[test]
    fn los_idiomas_usan_el_orden_basic_intermediate_advanced_native() {
        use Proficiency::*;
        let req = Requirements {
            languages: vec![rl("en", Some(Intermediate))],
            ..Default::default()
        };
        for (tiene, esperado) in [
            (vec![("en", Native)], 100),
            (vec![("en", Advanced)], 100),
            (vec![("en", Intermediate)], 100),
            (vec![("en", Basic)], 50),
            (vec![("fr", Native)], 0),
            (vec![], 0),
        ] {
            let m = compute(&perfil(&[], &tiene, vec![], None), &req, HOY);
            assert_eq!(m.score, esperado, "{tiene:?}");
        }
        let sin_minimo = Requirements {
            languages: vec![rl("en", None)],
            ..Default::default()
        };
        let m = compute(
            &perfil(&[], &[("en", Basic)], vec![], None),
            &sin_minimo,
            HOY,
        );
        assert_eq!(m.score, 100);
    }

    #[test]
    fn la_experiencia_es_proporcional_y_se_topa_en_los_anos_pedidos() {
        let req = Requirements {
            required_years: Some(4),
            ..Default::default()
        };
        let casos = [
            (vec![per((2024, 11, 11), Some((2026, 11, 11)))], 50), // 2 de 4
            (vec![per((2016, 1, 1), Some((2026, 1, 1)))], 100),    // de sobra
            (vec![], 0),
            (vec![per((2026, 11, 11), None)], 0), // empezó hoy
        ];
        for (exp, esperado) in casos {
            let m = compute(&perfil(&[], &[], exp.clone(), None), &req, HOY);
            assert_eq!(m.score, esperado, "{exp:?}");
        }
    }

    #[test]
    fn los_periodos_solapados_no_se_suman_dos_veces() {
        let req = Requirements {
            required_years: Some(4),
            ..Default::default()
        };
        // Dos years_f, de 2023-11-11 a 2025-11-11, contados en paralelo por dos trabajos.
        let exp = vec![
            per((2023, 11, 11), Some((2025, 11, 11))),
            per((2023, 11, 11), Some((2025, 11, 11))),
            per((2024, 6, 1), Some((2025, 1, 1))),
        ];
        let m = compute(&perfil(&[], &[], exp, None), &req, HOY);
        assert_eq!(m.score, 50);
        assert_eq!(comp(&m, "experience").years, Some(2.0));
        // Dos periodos que no se tocan sí suman: 1 + 1 years_f.
        let exp = vec![
            per((2020, 1, 1), Some((2021, 1, 1))),
            per((2022, 1, 1), Some((2023, 1, 1))),
        ];
        let m = compute(&perfil(&[], &[], exp, None), &req, HOY);
        assert_eq!(m.score, 50);
    }

    #[test]
    fn un_empleo_actual_cuenta_hasta_hoy_y_uno_futuro_no_cuenta() {
        let req = Requirements {
            required_years: Some(2),
            ..Default::default()
        };
        let m = compute(
            &perfil(&[], &[], vec![per((2024, 11, 11), None)], None),
            &req,
            HOY,
        );
        assert_eq!(m.score, 100);
        let m = compute(
            &perfil(&[], &[], vec![per((2027, 1, 1), Some((2028, 1, 1)))], None),
            &req,
            HOY,
        );
        assert_eq!(m.score, 0);
    }

    #[test]
    fn el_nivel_academico_es_completo_o_nada_por_sort_order() {
        let req = Requirements {
            academic_min: Some(3),
            ..Default::default()
        };
        for (tiene, esperado) in [(Some(3), 100), (Some(5), 100), (Some(2), 0), (None, 0)] {
            let m = compute(&perfil(&[], &[], vec![], tiene), &req, HOY);
            assert_eq!(m.score, esperado, "{tiene:?}");
        }
    }

    #[test]
    fn los_pesos_son_50_15_20_15_y_suman_100() {
        let req = Requirements {
            skills: vec![rs("rust", true, None)],
            languages: vec![rl("en", None)],
            required_years: Some(2),
            academic_min: Some(1),
        };
        let m = compute(&Profile::empty(), &req, HOY);
        let pesos: Vec<(&str, u32)> = m.components.iter().map(|c| (c.key, c.weight)).collect();
        assert_eq!(
            pesos,
            [
                ("skills", 50),
                ("languages", 15),
                ("experience", 20),
                ("academic_level", 15)
            ]
        );
        assert_eq!(pesos.iter().map(|p| p.1).sum::<u32>(), 100);
        // Cumplir todo da 100 exacto: los puntos son los pesos.
        let todo = perfil(
            &[("rust", None)],
            &[("en", Proficiency::Basic)],
            vec![per((2020, 1, 1), None)],
            Some(1),
        );
        let m = compute(&todo, &req, HOY);
        assert_eq!(m.score, 100);
        let puntos: f64 = m.components.iter().map(|c| c.points).sum();
        assert_eq!(puntos, 100.0);
    }

    #[test]
    fn lo_que_la_oferta_no_pide_no_aplica_y_su_peso_se_reparte() {
        // Solo pide experiencia: cumplirla da 100, no 20.
        let req = Requirements {
            required_years: Some(1),
            ..Default::default()
        };
        let m = compute(
            &perfil(&[], &[], vec![per((2020, 1, 1), None)], None),
            &req,
            HOY,
        );
        assert_eq!(m.score, 100);
        let c = comp(&m, "skills");
        assert!(!c.applies && c.points == 0.0 && c.weight == 50);
        assert_eq!(c.reason, Some("no_pedido"));
        // Pide habilidades (50) y nivel (15); cumple solo las habilidades: 50/65.
        let req = Requirements {
            skills: vec![rs("rust", true, None)],
            academic_min: Some(9),
            ..Default::default()
        };
        let m = compute(&perfil(&[("rust", None)], &[], vec![], Some(1)), &req, HOY);
        assert_eq!(m.score, 77);
    }

    #[test]
    fn sin_requisitos_comparables_el_puntaje_es_cero_con_motivo() {
        let m = compute(&Profile::empty(), &Requirements::default(), HOY);
        assert_eq!(m.score, 0);
        assert_eq!(m.reason, Some("sin_requisitos_comparables"));
        assert!(m.components.iter().all(|c| !c.applies));
        // Experiencia exigida pero de 0 years_f: tampoco es comparable (el servidor la deja en None).
        assert_eq!(
            m.to_json()["breakdown"]["reason"],
            "sin_requisitos_comparables"
        );
    }

    #[test]
    fn ma10_un_perfil_vacio_da_puntaje_bajo_explicado_sin_nan() {
        let req = Requirements {
            skills: vec![rs("rust", true, None)],
            languages: vec![rl("en", None)],
            required_years: Some(3),
            academic_min: Some(2),
        };
        let m = compute(&Profile::empty(), &req, HOY);
        assert_eq!(m.score, 0);
        assert!(m.empty_profile);
        assert_eq!(comp(&m, "skills").missing.len(), 1);
        assert_eq!(comp(&m, "experience").years, Some(0.0));
        let j = m.to_json().to_string();
        assert!(!j.contains("NaN") && !j.contains("null,\"points\""));
        for c in &m.components {
            assert!(c.points.is_finite());
        }
        // Una persona con algo no es «perfil vacío».
        let m = compute(&perfil(&[("x", None)], &[], vec![], None), &req, HOY);
        assert!(!m.empty_profile);
    }

    #[test]
    fn ma1_mismo_perfil_y_oferta_mismo_puntaje_y_desglose() {
        let req = Requirements {
            skills: vec![
                rs("rust", true, Some(Level::Advanced)),
                rs("sql", false, None),
            ],
            languages: vec![rl("en", Some(Proficiency::Advanced))],
            required_years: Some(3),
            academic_min: Some(2),
        };
        let p = perfil(
            &[("rust", Some(Level::Intermediate))],
            &[("en", Proficiency::Basic)],
            vec![per((2024, 1, 1), None)],
            Some(2),
        );
        let (a, b) = (compute(&p, &req, HOY), compute(&p, &req, HOY));
        assert_eq!(a, b);
        assert_eq!(a.to_json(), b.to_json());
    }

    #[test]
    fn ma2_el_desglose_dice_que_coincide_que_falta_y_cuanto_pesa() {
        let req = Requirements {
            skills: vec![
                rs("rust", true, None),
                rs("go", true, Some(Level::Expert)),
                rs("sql", false, None),
            ],
            ..Default::default()
        };
        let p = perfil(
            &[("rust", None), ("go", Some(Level::Beginner))],
            &[],
            vec![],
            None,
        );
        let m = compute(&p, &req, HOY);
        let j = m.to_json();
        let s = &j["breakdown"]["components"][0];
        assert_eq!(s["key"], "skills");
        assert_eq!(s["weight"], 50);
        assert_eq!(s["applies"], true);
        assert_eq!(s["matched"][0]["skill"]["code"], "rust");
        assert_eq!(s["matched"][0]["required"], true);
        assert_eq!(s["partial"][0]["skill"]["code"], "go");
        assert_eq!(s["partial"][0]["min_level"], "expert");
        assert_eq!(s["partial"][0]["level"], "beginner");
        assert_eq!(s["missing"][0]["skill"]["code"], "sql");
        assert_eq!(s["missing"][0]["required"], false);
        // 2 + 1 (mitad de 2) de 5 → 3 de 5 de 50.
        assert_eq!(s["points"], 30.0);
        assert_eq!(j["score"], 60);
    }

    #[test]
    fn el_redondeo_del_puntaje_es_al_entero_mas_cercano() {
        // 1 de 3 puntos de habilidades = 33.3 → 33; 2 de 3 = 66.6 → 67.
        let req = Requirements {
            skills: vec![rs("a", true, None), rs("b", false, None)],
            ..Default::default()
        };
        assert_eq!(
            compute(&perfil(&[("b", None)], &[], vec![], None), &req, HOY).score,
            33
        );
        assert_eq!(
            compute(&perfil(&[("a", None)], &[], vec![], None), &req, HOY).score,
            67
        );
    }

    // ---- MA4 / MA5: lo que el matching no puede leer

    fn vista(extra: Value) -> Value {
        let mut v = json!({
            "display_name": "Ana P.",
            "skills": [{"skill": {"id": "id-rust", "code": "rust", "name": "RUST"}, "level": "advanced", "years_of_experience": 3}],
            "experience": [{"company": "X", "title": "Dev", "start_date": "2022-11-11", "end_date": null}],
            "languages": [{"language": {"id": "id-en", "code": "en", "name": "EN"}, "proficiency": "native"}],
            "education": {"academic_level": {"id": "lvl-2", "code": "u", "name": "U"}, "profession": null, "job_hierarchy": null},
        });
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            for (k, x) in e {
                o.insert(k.clone(), x.clone());
            }
        }
        v
    }
    fn orden(id: &str) -> Option<i32> {
        (id == "lvl-2").then_some(2)
    }
    fn pide_todo() -> Requirements {
        Requirements {
            skills: vec![rs("rust", true, Some(Level::Advanced))],
            languages: vec![rl("en", Some(Proficiency::Advanced))],
            required_years: Some(8),
            academic_min: Some(2),
        }
    }

    #[test]
    fn ma4_ma5_edad_genero_y_demas_datos_no_cambian_el_puntaje() {
        let base = compute(
            &Profile::from_shared(&vista(json!({})), orden),
            &pide_todo(),
            HOY,
        );
        assert!(base.score > 0 && base.score < 100, "{}", base.score);
        let prohibidos = [
            ("age", json!(19)),
            ("age", json!(64)),
            ("birth_date", json!("1960-01-01")),
            ("date_of_birth", json!("2008-01-01")),
            ("gender", json!("f")),
            ("gender", json!("m")),
            ("marital_status", json!("casada")),
            ("married_last_name", json!("Gómez")),
            ("photo", json!("https://x/y.png")),
            ("avatar_url", json!("https://x/y.png")),
            ("address", json!("Zona 10")),
            ("email", json!("a@b.c")),
            ("phone", json!("5555")),
            ("display_name", json!("Zoe Zeta")),
            ("profile_description", json!("texto")),
        ];
        for (k, v) in prohibidos {
            let p = Profile::from_shared(&vista(json!({ k: v.clone() })), orden);
            let m = compute(&p, &pide_todo(), HOY);
            assert_eq!(m, base, "{k}={v}");
        }
    }

    #[test]
    fn el_codigo_del_matching_no_nombra_datos_demograficos() {
        // Solo el código de producción de este módulo (hasta `mod tests`), sin los comentarios.
        let fuentes = [include_str!("mod.rs"), include_str!("store.rs")];
        let prohibidas = [
            "birth",
            "age",
            "gender",
            "genero",
            "género",
            "marital",
            "married",
            "civil",
            "photo",
            "avatar",
            "address",
            "direccion",
            "nationality",
        ];
        for fuente in fuentes {
            let codigo = fuente.split("#[cfg(test)]").next().unwrap();
            for (n, linea) in codigo.lines().enumerate() {
                let l = linea.trim();
                if l.starts_with("//") {
                    continue;
                }
                let l = l.to_lowercase();
                // `age` como palabra, no como parte de otra («usage», «package», «message»).
                let palabras: Vec<&str> = l
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .collect();
                for p in prohibidas {
                    assert!(
                        !palabras.iter().any(|w| w.split('_').any(|x| x == p)),
                        "línea {}: «{p}» en {linea}",
                        n + 1
                    );
                }
            }
        }
    }

    // ---- el perfil compartido (lado reclutador)

    #[test]
    fn from_shared_lee_solo_lo_que_vino_y_lo_demas_queda_oculto() {
        let v = json!({"skills": [], "experience": []});
        let p = Profile::from_shared(&v, orden);
        assert_eq!(p.skills, Data::Known(vec![]));
        assert_eq!(p.experience, Data::Known(vec![]));
        assert_eq!(p.languages, Data::Hidden);
        assert_eq!(p.academic_sort, Data::Hidden);
        let p = Profile::from_shared(&vista(json!({})), orden);
        assert_eq!(p.academic_sort, Data::Known(Some(2)));
        assert_eq!(
            p.languages,
            Data::Known(vec![("id-en".into(), Proficiency::Native)])
        );
    }

    #[test]
    fn un_componente_con_dato_oculto_no_aplica_y_no_cuenta_como_faltante() {
        let v = json!({"skills": [{"skill": {"id": "id-rust"}, "level": "advanced"}],
                       "experience": []});
        let p = Profile::from_shared(&v, orden);
        let m = compute(&p, &pide_todo(), HOY);
        for k in ["languages", "academic_level"] {
            let c = comp(&m, k);
            assert!(!c.applies, "{k}");
            assert_eq!(c.reason, Some("no_compartido"));
            assert!(c.missing.is_empty());
        }
        // Skills (50, completo) y experiencia (20, 0 de 8 years_f): 50 / 70.
        assert_eq!(m.score, 71);
        // Lo mismo con todo a la vista da otra cosa: los idiomas y el nivel sí suman.
        let todo = compute(
            &Profile::from_shared(&vista(json!({})), orden),
            &pide_todo(),
            HOY,
        );
        assert_ne!(todo.score, m.score);
    }

    #[test]
    fn from_shared_ignora_lo_mal_formado_sin_fallar() {
        let v = json!({"skills": [{"skill": 5}, 7, {"skill": {"id": "id-rust"}, "level": "raro"}],
                       "experience": [{"start_date": "no"}, {"start_date": "2020-02-30"}],
                       "languages": "x", "education": 3});
        let p = Profile::from_shared(&v, orden);
        assert_eq!(p.skills, Data::Known(vec![("id-rust".into(), None)]));
        assert_eq!(p.experience, Data::Known(vec![]));
        let _ = compute(&p, &pide_todo(), HOY);
    }
}

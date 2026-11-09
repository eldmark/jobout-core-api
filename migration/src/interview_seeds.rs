//! Datos semilla del catálogo de la entrevista (Fase 3, paso 1): el elenco de recruiters, los
//! cinco niveles y la rúbrica inicial de cada recruiter.
//!
//! El código se refiere a un recruiter por su `code` (`RecruiterCode`), nunca por un id: los ids
//! son UUID v7 generados al migrar y cambian entre entornos.

/// Los `code` estables de `ai_recruiters`. Son la única forma de nombrar a un recruiter en el código.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecruiterCode {
    Ana,
    Martin,
    Laura,
    Carlos,
    Sofia,
}

impl RecruiterCode {
    /// Todos, en el orden en que se muestran.
    pub const ALL: [RecruiterCode; 5] = [
        RecruiterCode::Ana,
        RecruiterCode::Martin,
        RecruiterCode::Laura,
        RecruiterCode::Carlos,
        RecruiterCode::Sofia,
    ];

    /// El valor de la columna `ai_recruiters.code`.
    pub const fn as_str(self) -> &'static str {
        match self {
            RecruiterCode::Ana => "ana",
            RecruiterCode::Martin => "martin",
            RecruiterCode::Laura => "laura",
            RecruiterCode::Carlos => "carlos",
            RecruiterCode::Sofia => "sofia",
        }
    }

    /// Lo contrario de `as_str`: `None` si el texto no es un recruiter conocido.
    pub fn parse(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == code)
    }
}

/// Códigos de los criterios de toda rúbrica.
pub const CRITERIA_CODES: [&str; 5] = [
    "especificidad",
    "estructura",
    "relevancia",
    "claridad",
    "profundidad",
];

/// Un criterio de la rúbrica: (code, name, max_points, description).
pub type Criterion = (&'static str, &'static str, u32, &'static str);

/// Un recruiter sembrado.
pub struct RecruiterSeed {
    pub code: RecruiterCode,
    pub display_name: &'static str,
    pub role_title: &'static str,
    pub personality: &'static str,
    /// `languages.code` en el que entrevista.
    pub language: &'static str,
    pub criteria: [Criterion; 5],
}

const D_ESP: &str =
    "Ejemplos concretos: números, nombres propios, herramientas y resultados medibles.";
const D_EST: &str = "Respuesta ordenada: situación, acción y resultado.";
const D_REL: &str = "La respuesta contesta lo que se preguntó y aporta al puesto.";
const D_CLA: &str = "Ideas claras, directas y fáciles de seguir, sin rodeos ni muletillas.";
const D_PRO: &str = "Explica el porqué de sus decisiones y aguanta la repregunta.";

const fn criteria(esp: u32, est: u32, rel: u32, cla: u32, pro: u32) -> [Criterion; 5] {
    [
        ("especificidad", "Especificidad", esp, D_ESP),
        ("estructura", "Estructura", est, D_EST),
        ("relevancia", "Relevancia", rel, D_REL),
        ("claridad", "Claridad", cla, D_CLA),
        ("profundidad", "Profundidad", pro, D_PRO),
    ]
}

/// El elenco (ver `producto.md`). El peso de cada criterio refleja lo que ese recruiter mira.
pub const RECRUITERS: [RecruiterSeed; 5] = [
    RecruiterSeed {
        code: RecruiterCode::Ana,
        display_name: "Ana",
        role_title: "RRHH",
        personality: "Muy amable. La primera entrevista de todos.",
        language: "es",
        criteria: criteria(20, 15, 25, 25, 15),
    },
    RecruiterSeed {
        code: RecruiterCode::Martin,
        display_name: "Martín",
        role_title: "CTO",
        personality: "Exigente. Técnico, repregunta hasta el fondo.",
        language: "es",
        criteria: criteria(30, 15, 20, 10, 25),
    },
    RecruiterSeed {
        code: RecruiterCode::Laura,
        display_name: "Laura",
        role_title: "Recruiter Big Tech",
        personality: "Estructurada, formato de entrevista conductual.",
        language: "es",
        criteria: criteria(20, 35, 15, 15, 15),
    },
    RecruiterSeed {
        code: RecruiterCode::Carlos,
        display_name: "Carlos",
        role_title: "CEO de startup",
        personality: "Informal, va al grano, pregunta por ambición y riesgo.",
        language: "es",
        criteria: criteria(25, 10, 25, 25, 15),
    },
    RecruiterSeed {
        code: RecruiterCode::Sofia,
        display_name: "Sofía",
        role_title: "Recruiter internacional",
        personality: "Entrevista en inglés.",
        language: "en",
        criteria: criteria(20, 20, 20, 25, 15),
    },
];

/// Un nivel sembrado, de menor a mayor. El previo de cada uno es el anterior de la lista.
pub struct LevelSeed {
    pub code: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    /// Score mínimo en el nivel previo; `None` en el primero.
    pub min_score_to_unlock: Option<i16>,
    /// Solo los niveles con banco de preguntas se pueden empezar.
    pub available: bool,
}

pub const LEVELS: [LevelSeed; 5] = [
    LevelSeed {
        code: "junior",
        display_name: "Junior",
        description: "Lo básico, sin presión.",
        min_score_to_unlock: None,
        available: true,
    },
    LevelSeed {
        code: "startup",
        display_name: "Startup",
        description: "Ritmo rápido y preguntas de ambición y riesgo.",
        min_score_to_unlock: Some(60),
        available: true,
    },
    LevelSeed {
        code: "amazon",
        display_name: "Amazon",
        description: "Entrevista conductual por principios de liderazgo.",
        min_score_to_unlock: Some(70),
        available: false,
    },
    LevelSeed {
        code: "google",
        display_name: "Google",
        description: "Resolución de problemas y razonamiento en voz alta.",
        min_score_to_unlock: Some(75),
        available: false,
    },
    LevelSeed {
        code: "mckinsey",
        display_name: "McKinsey",
        description: "Casos de negocio con estructura y cuantificación.",
        min_score_to_unlock: Some(80),
        available: false,
    },
];

/// Los criterios como el `jsonb` de `rubric_versions.criteria`. El texto sembrado no lleva
/// comillas ni barras (lo vigila una prueba), así que no necesita escape.
pub fn criteria_json(criteria: &[Criterion]) -> String {
    let items: Vec<String> = criteria
        .iter()
        .map(|(code, name, pts, desc)| {
            format!(
                "{{\"code\":\"{code}\",\"name\":\"{name}\",\"max_points\":{pts},\"description\":\"{desc}\"}}"
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// K8: los criterios de cada rúbrica suman exactamente 100 y traen sus cuatro campos.
    #[test]
    fn k8_los_criterios_suman_100() {
        for r in &RECRUITERS {
            let total: u32 = r.criteria.iter().map(|c| c.2).sum();
            assert_eq!(total, 100, "{}", r.code.as_str());
            let codes: Vec<&str> = r.criteria.iter().map(|c| c.0).collect();
            assert_eq!(codes, CRITERIA_CODES, "{}", r.code.as_str());
            for (code, name, pts, desc) in r.criteria {
                assert!(!code.is_empty() && !name.is_empty() && !desc.is_empty());
                assert!(pts > 0, "{code} sin puntos");
            }
        }
    }

    /// El JSON sembrado se arma a mano: el texto no puede romperlo ni el SQL.
    #[test]
    fn el_texto_sembrado_no_necesita_escape() {
        let mut textos = vec![];
        for r in &RECRUITERS {
            textos.extend([r.display_name, r.role_title, r.personality]);
            for c in r.criteria {
                textos.extend([c.1, c.3]);
            }
        }
        for l in &LEVELS {
            textos.extend([l.display_name, l.description]);
        }
        for t in textos {
            assert!(!t.contains(['"', '\\', '\'']), "{t}");
        }
    }

    /// K10: los códigos son únicos, ASCII en minúsculas, y se recorren de ida y vuelta.
    #[test]
    fn k10_los_codigos_son_estables() {
        let mut vistos = std::collections::HashSet::new();
        for c in RecruiterCode::ALL {
            let s = c.as_str();
            assert!(s.bytes().all(|b| b.is_ascii_lowercase()), "{s}");
            assert!(vistos.insert(s), "{s} repetido");
            assert_eq!(RecruiterCode::parse(s), Some(c));
        }
        assert_eq!(RecruiterCode::parse("nadie"), None);
        assert_eq!(
            RecruiterCode::ALL.map(RecruiterCode::as_str),
            ["ana", "martin", "laura", "carlos", "sofia"]
        );
        // El elenco sembrado está en el mismo orden y no le falta nadie.
        assert_eq!(RECRUITERS.map(|r| r.code), RecruiterCode::ALL);
    }

    #[test]
    fn solo_sofia_entrevista_en_ingles() {
        for r in &RECRUITERS {
            let esperado = if r.code == RecruiterCode::Sofia {
                "en"
            } else {
                "es"
            };
            assert_eq!(r.language, esperado);
        }
    }

    #[test]
    fn el_json_de_criterios_es_el_esperado() {
        let j = criteria_json(&[("a", "A", 100, "d")]);
        assert_eq!(
            j,
            r#"[{"code":"a","name":"A","max_points":100,"description":"d"}]"#
        );
    }
}

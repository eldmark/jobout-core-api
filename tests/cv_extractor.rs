//! Casos de verdad del extractor determinista (D1–D11 de docs/working/fase2-perfil-cv.md).
//!
//! Se escribieron antes que la implementación, en rojo.

use std::time::Instant;

use jobout_core_api::cv::extractor::{
    Confidence, CvExtractor, DeterministicExtractor, LanguageEntry, Proficiency, Proposal,
    SkillEntry,
};
use serde_json::json;

fn skill(id: &str, code: &str, name: &str, aliases: &[&str]) -> SkillEntry {
    SkillEntry {
        id: id.into(),
        code: code.into(),
        name: name.into(),
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
    }
}

fn skills() -> Vec<SkillEntry> {
    vec![
        skill("s-rust", "rust", "Rust", &[]),
        skill("s-pg", "postgresql", "PostgreSQL", &["postgres", "psql"]),
        skill("s-docker", "docker", "Docker", &[]),
        skill("s-js", "javascript", "JavaScript", &["js"]),
        skill("s-prog", "programming", "Programación", &[]),
        skill("s-node", "nodejs", "Node.js", &[]),
        skill("s-cpp", "cpp", "C++", &[]),
        skill("s-ml", "machine-learning", "Machine Learning", &["ml"]),
    ]
}

fn languages() -> Vec<LanguageEntry> {
    [
        ("l-en", "en", "Inglés"),
        ("l-es", "es", "Español"),
        ("l-fr", "fr", "Francés"),
    ]
    .iter()
    .map(|(id, code, name)| LanguageEntry {
        id: (*id).into(),
        code: (*code).into(),
        name: (*name).into(),
    })
    .collect()
}

fn extractor() -> DeterministicExtractor {
    DeterministicExtractor::new(skills(), languages())
}

fn extract(text: &str) -> Proposal {
    extractor().extract(text)
}

fn skill_ids(p: &Proposal) -> Vec<&str> {
    p.skills.iter().map(|s| s.skill_id.as_str()).collect()
}

// ---------- D1–D4: skills ----------

/// D1
#[test]
fn d1_propone_las_skills_mencionadas_con_su_id() {
    let p = extract("Experiencia con Rust, PostgreSQL y Docker en producción.");
    assert_eq!(skill_ids(&p), ["s-docker", "s-pg", "s-rust"]);
    let rust = p.skills.iter().find(|s| s.skill_id == "s-rust").unwrap();
    assert_eq!((rust.code.as_str(), rust.name.as_str()), ("rust", "Rust"));
    assert_eq!(rust.confidence, Confidence::High);
}

/// D2
#[test]
fn d2_ignora_mayusculas_y_tildes() {
    let p = extract("POSTGRESQL y programacion; también PROGRAMACIÓN y rust");
    assert_eq!(skill_ids(&p), ["s-pg", "s-prog", "s-rust"]);
}

/// D3
#[test]
fn d3_no_inventa_por_subcadena() {
    let p = extract("Trabajé con rustico y javascripty, dockerizado, trustworthy, ml5js");
    assert!(p.skills.is_empty(), "{:?}", p.skills);
}

#[test]
fn d3_separadores_no_alfanumericos_si_delimitan_palabra() {
    let p = extract("(Rust), docker/postgresql; rust-lang");
    assert_eq!(skill_ids(&p), ["s-docker", "s-pg", "s-rust"]);
}

/// D4
#[test]
fn d4_reconoce_alias() {
    let p = extract("Frontend con JS y bases de datos con postgres");
    assert_eq!(skill_ids(&p), ["s-js", "s-pg"]);
}

#[test]
fn skills_con_simbolos_y_varias_palabras() {
    let p = extract("Node.js, C++ y machine   learning.\nTambién Machine\tLearning.");
    assert_eq!(skill_ids(&p), ["s-cpp", "s-ml", "s-node"]);
}

#[test]
fn una_skill_se_propone_una_sola_vez() {
    let p = extract("Rust Rust rust RUST");
    assert_eq!(p.skills.len(), 1);
}

#[test]
fn skill_por_nombre_y_alias_prevalece_el_nombre() {
    let p = extract("JavaScript (JS)");
    assert_eq!(p.skills.len(), 1);
    assert_eq!(p.skills[0].confidence, Confidence::High);
}

// ---------- D5: idiomas ----------

fn lang<'a>(p: &'a Proposal, id: &str) -> &'a jobout_core_api::cv::extractor::LanguageProposal {
    p.languages.iter().find(|l| l.language_id == id).unwrap()
}

/// D5
#[test]
fn d5_idioma_con_nivel() {
    let p = extract("Idiomas\nInglés avanzado\nEnglish – intermediate\n");
    assert_eq!(p.languages.len(), 1, "inglés e English son el mismo idioma");
    let en = lang(&p, "l-en");
    assert_eq!(en.proficiency, Proficiency::Advanced);
    assert!(!en.to_confirm);
    assert_eq!(en.code, "en");
    assert_eq!(en.name, "Inglés");
}

#[test]
fn d5_english_intermediate() {
    let p = extract("English – intermediate");
    let en = lang(&p, "l-en");
    assert_eq!(en.proficiency, Proficiency::Intermediate);
    assert!(!en.to_confirm);
}

#[test]
fn d5_sin_nivel_es_intermedio_a_confirmar() {
    let p = extract("Hablo francés con mis colegas.");
    let fr = lang(&p, "l-fr");
    assert_eq!(fr.proficiency, Proficiency::Intermediate);
    assert!(fr.to_confirm);
    assert_ne!(fr.confidence, Confidence::High);
}

#[test]
fn d5_varios_idiomas_en_una_linea_cada_uno_con_su_nivel() {
    let p = extract("Idiomas: Español nativo, Inglés avanzado, Francés básico");
    assert_eq!(lang(&p, "l-es").proficiency, Proficiency::Native);
    assert_eq!(lang(&p, "l-en").proficiency, Proficiency::Advanced);
    assert_eq!(lang(&p, "l-fr").proficiency, Proficiency::Basic);
    assert!(p.languages.iter().all(|l| !l.to_confirm));
}

#[test]
fn d5_niveles_en_ingles_y_marco_europeo() {
    let p = extract("Spanish: native\nEnglish (C1)\nFrench - A2");
    assert_eq!(lang(&p, "l-es").proficiency, Proficiency::Native);
    assert_eq!(lang(&p, "l-en").proficiency, Proficiency::Advanced);
    assert_eq!(lang(&p, "l-fr").proficiency, Proficiency::Basic);
}

#[test]
fn d5_la_palabra_en_no_es_el_idioma_ingles() {
    // «en» es el código de Inglés, pero también una preposición.
    let p = extract("Trabajo en una empresa en Madrid.");
    assert!(p.languages.is_empty(), "{:?}", p.languages);
}

// ---------- D6: experiencia ----------

#[test]
fn d6_fechas_solo_anio() {
    let p = extract("Experiencia\nDesarrollador Backend en Acme\n2021 – 2023\n");
    assert_eq!(p.experiences.len(), 1);
    let e = &p.experiences[0];
    assert_eq!(e.company, "Acme");
    assert_eq!(e.title, "Desarrollador Backend");
    assert_eq!(e.start_date, "2021-01-01");
    assert_eq!(e.end_date.as_deref(), Some("2023-12-31"));
}

#[test]
fn d6_mes_abreviado_y_actual() {
    let p = extract("Experiencia\nAcme | Backend Developer | ene 2021 – actual\n- Servicios en Rust\n- Migración a PostgreSQL");
    let e = &p.experiences[0];
    assert_eq!(e.company, "Acme");
    assert_eq!(e.title, "Backend Developer");
    assert_eq!(e.start_date, "2021-01-01");
    assert_eq!(e.end_date, None);
    assert_eq!(
        e.description.as_deref(),
        Some("Servicios en Rust Migración a PostgreSQL")
    );
}

#[test]
fn d6_mes_numerico() {
    let p = extract("Experiencia\nGlobex - Software Engineer 01/2021 – 03/2023");
    let e = &p.experiences[0];
    assert_eq!(
        (e.company.as_str(), e.title.as_str()),
        ("Globex", "Software Engineer")
    );
    assert_eq!(e.start_date, "2021-01-01");
    assert_eq!(e.end_date.as_deref(), Some("2023-03-31"));
}

#[test]
fn d6_actual_presente_hoy_dejan_end_date_nulo() {
    for fin in ["actual", "presente", "hoy", "Present", "Actualidad"] {
        let p = extract(&format!("Experiencia\nAcme | Dev | 03/2020 - {fin}"));
        assert_eq!(p.experiences.len(), 1, "{fin}");
        assert_eq!(p.experiences[0].end_date, None, "{fin}");
    }
}

#[test]
fn d6_meses_en_espanol_e_ingles_y_de() {
    let p = extract("Experiencia\nAcme | Dev | enero de 2019 – Dec 2020\nBeta | Analista | Sept 2017 – abr 2018");
    assert_eq!(p.experiences[0].start_date, "2019-01-01");
    assert_eq!(p.experiences[0].end_date.as_deref(), Some("2020-12-31"));
    assert_eq!(p.experiences[1].start_date, "2017-09-01");
    assert_eq!(p.experiences[1].end_date.as_deref(), Some("2018-04-30"));
}

#[test]
fn d6_fin_de_febrero_en_anio_bisiesto() {
    let p = extract("Experiencia\nAcme | Dev | 02/2019 - 02/2020\nB | Dev | 02/2020 - 02/2021");
    assert_eq!(p.experiences[0].end_date.as_deref(), Some("2020-02-29"));
    assert_eq!(p.experiences[1].end_date.as_deref(), Some("2021-02-28"));
}

#[test]
fn d6_varios_bloques_cada_descripcion_en_su_bloque() {
    let txt = "Experiencia laboral\n\
        Acme | Backend Developer | 2021 – 2023\n\
        - Hice APIs\n\
        Globex | Data Analyst | 2018 – 2020\n\
        - Hice reportes\n\
        Educación\n\
        Universidad Nacional\n\
        2012 – 2017\n";
    let p = extract(txt);
    assert_eq!(p.experiences.len(), 2, "{:?}", p.experiences);
    assert_eq!(p.experiences[0].company, "Acme");
    assert_eq!(p.experiences[0].description.as_deref(), Some("Hice APIs"));
    assert_eq!(p.experiences[1].company, "Globex");
    assert_eq!(
        p.experiences[1].description.as_deref(),
        Some("Hice reportes")
    );
}

#[test]
fn d6_cargo_y_empresa_en_lineas_previas_a_la_fecha() {
    let p = extract("Experiencia\nBackend Developer\nAcme\nene 2021 – actual");
    let e = &p.experiences[0];
    assert_eq!(
        (e.company.as_str(), e.title.as_str()),
        ("Acme", "Backend Developer")
    );
}

#[test]
fn d6_sin_encabezado_tambien_encuentra_bloques() {
    let p = extract("Acme | Backend Developer | ene 2021 – actual");
    assert_eq!(p.experiences.len(), 1);
}

#[test]
fn d6_fechas_invertidas_no_son_experiencia() {
    let p = extract("Experiencia\nAcme | Dev | 2023 – 2021");
    assert!(p.experiences.is_empty());
}

#[test]
fn d6_anios_absurdos_no_son_fechas() {
    let p = extract("Experiencia\nAcme | Dev | 1234 – 9999");
    assert!(p.experiences.is_empty());
}

// ---------- D7 ----------

#[test]
fn d7_texto_vacio_o_sin_nada_reconocible_da_listas_vacias() {
    for t in [
        "",
        "   \n\t\n",
        "Lorem ipsum dolor sit amet.",
        "12345 !!! ???",
    ] {
        let p = extract(t);
        assert_eq!(p, Proposal::default(), "{t:?}");
        assert!(p.skills.is_empty() && p.languages.is_empty() && p.experiences.is_empty());
        assert_eq!(p.description, None);
    }
}

// ---------- D8: sin datos de contacto ----------

const CV_CON_CONTACTO: &str = "Ana Pérez\n\
Email: ana.perez@example.com\n\
Tel: +34 612 345 678\n\
Móvil (555) 123-4567\n\
Teléfono: 612345678\n\
Dirección: Calle Mayor 12, 28013 Madrid\n\
Av. Libertador 1234, Buenos Aires\n\
LinkedIn: linkedin.com/in/anaperez   https://github.com/anaperez   www.anaperez.dev\n\
Perfil\n\
Desarrolladora Rust con 5 años de experiencia. Escríbeme a ana.perez@example.com o llama al +34 612 345 678.\n\
Experiencia\n\
Acme | Backend Developer | 2021 – 2023 ana@acme.com +34 600 111 222\n\
- Contacto: 600111222 / soporte@acme.com\n\
- Servicios en Rust y Docker\n\
Idiomas\n\
Inglés avanzado\n";

const PROHIBIDOS: &[&str] = &[
    "@",
    "612",
    "345",
    "555",
    "123-4567",
    "600",
    "111 222",
    "Calle Mayor",
    "28013",
    "Libertador",
    "1234",
    "linkedin",
    "github",
    "www.",
    "example.com",
    "acme.com",
    "Móvil",
    "Tel",
];

/// D8
#[test]
fn d8_jamas_devuelve_contacto_y_no_rompe_lo_demas() {
    let p = extract(CV_CON_CONTACTO);
    let json = serde_json::to_string(&p).unwrap();
    for malo in PROHIBIDOS {
        assert!(!json.contains(malo), "el JSON contiene {malo:?}: {json}");
    }
    // Y lo legítimo sigue ahí.
    assert_eq!(skill_ids(&p), ["s-docker", "s-rust"]);
    assert_eq!(lang(&p, "l-en").proficiency, Proficiency::Advanced);
    assert_eq!(p.experiences.len(), 1, "{:?}", p.experiences);
    let e = &p.experiences[0];
    assert_eq!(
        (e.company.as_str(), e.title.as_str()),
        ("Acme", "Backend Developer")
    );
    assert_eq!(e.start_date, "2021-01-01");
    assert_eq!(e.end_date.as_deref(), Some("2023-12-31"));
    // La línea «Contacto: …» se descarta entera: solo traía datos de contacto.
    assert_eq!(e.description.as_deref(), Some("Servicios en Rust y Docker"));
    let d = p.description.expect("hay sección de perfil");
    assert!(d.starts_with("Desarrolladora Rust con 5 años"));
}

#[test]
fn d8_un_telefono_no_se_confunde_con_fechas() {
    // El rango de años (8 dígitos) no es un teléfono y debe sobrevivir.
    let p = extract("Experiencia\nAcme | Dev | 2021 - 2023");
    assert_eq!(p.experiences.len(), 1);
    assert_eq!(p.experiences[0].start_date, "2021-01-01");
}

#[test]
fn d8_description_no_se_toma_de_lineas_de_contacto() {
    let p = extract("Perfil\nTel: 600 111 222\nCorreo: a@b.co\nIngeniera de software.");
    assert_eq!(p.description.as_deref(), Some("Ingeniera de software."));
}

// ---------- descripción ----------

#[test]
fn description_es_el_inicio_de_la_seccion_de_perfil() {
    let p = extract("Ana\nResumen profesional\nIngeniera de software.\nMe gusta Rust.\nExperiencia\nAcme | Dev | 2020 – 2021");
    assert_eq!(
        p.description.as_deref(),
        Some("Ingeniera de software. Me gusta Rust.")
    );
}

#[test]
fn description_se_corta_a_300_caracteres() {
    let largo = "palabra ".repeat(100);
    let p = extract(&format!("Perfil\n{largo}\nExperiencia\n"));
    let d = p.description.unwrap();
    assert!(
        d.chars().count() <= 300 && d.chars().count() > 280,
        "{}",
        d.chars().count()
    );
    assert_eq!(d, d.trim());
}

#[test]
fn description_nula_sin_seccion_de_perfil() {
    assert_eq!(extract("Solo sé Rust").description, None);
}

#[test]
fn description_corta_en_limite_de_caracter_no_de_byte() {
    let p = extract(&format!("Perfil\n{}", "é".repeat(500)));
    assert_eq!(p.description.unwrap().chars().count(), 300);
}

// ---------- D9: confianza ----------

/// D9
#[test]
fn d9_alias_nunca_es_alta() {
    let p = extract("Uso js y postgres");
    assert!(p.skills.iter().all(|s| s.confidence != Confidence::High));
}

#[test]
fn d9_fecha_de_solo_anio_nunca_es_alta() {
    let p = extract("Experiencia\nAcme | Backend Developer | 2021 – 2023");
    assert_ne!(p.experiences[0].confidence, Confidence::High);
}

#[test]
fn d9_fecha_con_mes_y_roles_claros_es_alta() {
    let p = extract("Experiencia\nAcme | Backend Developer | 01/2021 – 03/2023");
    assert_eq!(p.experiences[0].confidence, Confidence::High);
}

#[test]
fn d9_roles_adivinados_son_baja() {
    let p = extract("Experiencia\nAlfa | Beta | 01/2021 – 03/2023");
    assert_eq!(p.experiences[0].confidence, Confidence::Low);
}

// ---------- D10: determinismo ----------

/// D10
#[test]
fn d10_misma_entrada_misma_salida() {
    let a = extract(CV_CON_CONTACTO);
    let b = extract(CV_CON_CONTACTO);
    assert_eq!(a, b);
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

#[test]
fn d10_el_orden_del_catalogo_no_cambia_el_resultado() {
    let mut s = skills();
    s.reverse();
    let mut l = languages();
    l.reverse();
    let otro = DeterministicExtractor::new(s, l);
    let txt = "Rust docker postgres js\nInglés avanzado, Francés";
    assert_eq!(otro.extract(txt), extract(txt));
}

#[test]
fn d10_salida_ordenada_por_nombre() {
    let p = extract("Rust, Docker, PostgreSQL, JavaScript, C++");
    let nombres: Vec<_> = p.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        nombres,
        ["C++", "Docker", "JavaScript", "PostgreSQL", "Rust"]
    );
}

// ---------- D11: rendimiento ----------

/// D11
#[test]
fn d11_dos_megas_de_texto_en_menos_de_dos_segundos() {
    let bloque = "Acme | Backend Developer | 01/2021 – 03/2023\n- Servicios en Rust, Docker y PostgreSQL con js\nInglés avanzado, rustico, javascripty 600 111 222 a@b.co\n";
    let texto = bloque.repeat(2 * 1024 * 1024 / bloque.len() + 1);
    assert!(texto.len() >= 2 * 1024 * 1024);
    let t0 = Instant::now();
    let p = extract(&texto);
    let dt = t0.elapsed();
    assert!(dt.as_secs_f64() < 2.0, "tardó {dt:?}");
    assert_eq!(p.skills.len(), 4);
    assert!(p.experiences.len() <= 30);
}

#[test]
fn d11_dos_megas_en_una_sola_linea_y_sin_separadores_utiles() {
    let casos: Vec<(&str, String)> = vec![
        ("rust x420k", "rust ".repeat(420_000)),
        ("1 x1M", "1 ".repeat(1_050_000)),
        ("una palabra", "a".repeat(2 * 1024 * 1024)),
        ("años", "2021 – ".repeat(350_000)),
    ];
    for (nombre, texto) in casos {
        let t0 = Instant::now();
        let p = extract(&texto);
        let dt = t0.elapsed();
        assert!(dt.as_secs_f64() < 2.0, "{nombre} tardó {dt:?}");
        match nombre {
            "rust x420k" => assert_eq!(p.skills.len(), 1),
            _ => assert!(p.skills.is_empty()),
        }
    }
}

// ---------- formato JSON del contrato ----------

#[test]
fn el_json_sigue_el_contrato() {
    let p = extract(
        "Perfil\nIngeniera.\nExperiencia\nAcme | Backend Developer | 01/2021 – actual\nRust y js\nIdiomas\nInglés avanzado, Francés",
    );
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(
        v,
        json!({
            "description": "Ingeniera.",
            "skills": [
                {"skill_id": "s-js", "code": "javascript", "name": "JavaScript", "confidence": "media"},
                {"skill_id": "s-rust", "code": "rust", "name": "Rust", "confidence": "alta"}
            ],
            "languages": [
                {"language_id": "l-fr", "code": "fr", "name": "Francés", "proficiency": "intermediate", "to_confirm": true, "confidence": "baja"},
                {"language_id": "l-en", "code": "en", "name": "Inglés", "proficiency": "advanced", "to_confirm": false, "confidence": "alta"}
            ],
            "experiences": [
                {"company": "Acme", "title": "Backend Developer", "start_date": "2021-01-01", "end_date": null, "description": "Rust y js", "confidence": "alta"}
            ]
        })
    );
}

#[test]
fn d8_una_linea_con_palabra_de_calle_y_fechas_es_experiencia_no_direccion() {
    let p = extract("Experiencia\nAvenida Corp | Backend Developer | 2021 – 2023");
    assert_eq!(p.experiences.len(), 1);
    assert_eq!(p.experiences[0].company, "Avenida Corp");
}

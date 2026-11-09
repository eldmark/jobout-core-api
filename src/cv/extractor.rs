//! Extractor determinista: del texto de un CV a una **propuesta** que la persona revisa.
//!
//! Sin red, sin modelos, sin azar: el mismo texto y el mismo catálogo dan siempre el mismo
//! resultado, en el mismo orden. El formato JSON de [`Proposal`] es el del contrato de la
//! Fase 2 (`extraction.proposal`).
//!
//! # Reglas que importan
//!
//! - **Nunca** devuelve teléfonos, emails, URLs de perfiles ni direcciones. Antes de extraer
//!   nada se depura el texto: se quitan esas palabras y se descartan las líneas que quedan
//!   solo con una etiqueta («Tel:», «Dirección: …»). Ver `redact_line`.
//! - Skills e idiomas coinciden por **palabra completa**, sin distinguir mayúsculas ni tildes.
//!   El catálogo se indexa una vez (mapa por primera palabra) y el texto se recorre una vez:
//!   no hay búsquedas cuadráticas.
//! - La confianza nunca es `alta` si viene de un alias o de una fecha de solo año.
//! - Fechas de solo año: `2021 – 2023` es `2021-01-01` a `2023-12-31` y su confianza es, como
//!   mucho, `media`. Con mes, el inicio es el día 1 y el fin el último día de ese mes.
//!
//! # Límites conocidos
//!
//! Es heurístico. Se equivoca con CVs de columnas o tablas (el texto llega desordenado), con
//! cargos sin palabra de cargo reconocible («Alfa | Beta» se adivina y sale con confianza
//! `baja`), con teléfonos de menos de 9 dígitos o pegados a un año sin `+`, y con direcciones
//! sin etiqueta ni palabra de calle. Un cargo en una línea con «Avenida … 2021» cuenta como
//! dirección solo si esa línea no trae un rango de fechas.

use std::collections::HashMap;

use serde::Serialize;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

const MAX_DESCRIPTION_CHARS: usize = 300;
const MAX_EXPERIENCE_DESCRIPTION_CHARS: usize = 500;
const MAX_HEADER_CHARS: usize = 150;
const MAX_SKILLS: usize = 60;
const MAX_LANGUAGES: usize = 10;
const MAX_EXPERIENCES: usize = 30;

// ---------- catálogo y propuesta ----------

#[derive(Debug, Clone)]
pub struct SkillEntry {
    pub id: String,
    pub code: String,
    pub name: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LanguageEntry {
    pub id: String,
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Confidence {
    #[serde(rename = "baja")]
    Low,
    #[serde(rename = "media")]
    Medium,
    #[serde(rename = "alta")]
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Proficiency {
    Native,
    Advanced,
    Intermediate,
    Basic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillProposal {
    pub skill_id: String,
    pub code: String,
    pub name: String,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LanguageProposal {
    pub language_id: String,
    pub code: String,
    pub name: String,
    pub proficiency: Proficiency,
    /// `true` si el CV no decía el nivel y se propuso `intermediate` por defecto.
    pub to_confirm: bool,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExperienceProposal {
    pub company: String,
    pub title: String,
    /// `YYYY-MM-DD`.
    pub start_date: String,
    /// `YYYY-MM-DD`, o `null` si el trabajo sigue (actual, presente, hoy).
    pub end_date: Option<String>,
    pub description: Option<String>,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Proposal {
    pub description: Option<String>,
    pub skills: Vec<SkillProposal>,
    pub languages: Vec<LanguageProposal>,
    pub experiences: Vec<ExperienceProposal>,
}

pub trait CvExtractor: Send + Sync {
    fn extract(&self, text: &str) -> Proposal;
}

// ---------- normalización ----------

/// Minúsculas y sin tildes («Programación» → «programacion», «Español» → «espanol»).
fn fold(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
        } else {
            for d in std::iter::once(c).nfd().filter(|d| !is_combining_mark(*d)) {
                out.extend(d.to_lowercase());
            }
        }
    }
    out
}

/// Palabras de un texto ya plegado. Una palabra es una racha alfanumérica; además admite
/// `+`/`#` finales (`c++`, `c#`), un punto entre alfanuméricos (`node.js`) y un punto inicial
/// pegado a letras (`.net`).
fn word_spans(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut it = s.char_indices().peekable();
    let mut prev_alnum = false;
    while let Some((start, c)) = it.next() {
        let leading_dot =
            c == '.' && !prev_alnum && it.peek().is_some_and(|(_, n)| n.is_alphabetic());
        if !(c.is_alphanumeric() || leading_dot) {
            prev_alnum = false;
            continue;
        }
        let mut end = start + c.len_utf8();
        while let Some(&(i, cj)) = it.peek() {
            if cj.is_alphanumeric() {
                end = i + cj.len_utf8();
                it.next();
            } else if cj == '.' && {
                let mut look = it.clone();
                look.next();
                look.peek().is_some_and(|(_, n)| n.is_alphanumeric())
            } {
                end = i + 1;
                it.next();
            } else {
                break;
            }
        }
        // Símbolos finales: solo si cierran la palabra (`c++` sí, `c+d` no).
        let mut look = it.clone();
        let (mut sym_end, mut count) = (end, 0);
        while count < 2 {
            match look.peek() {
                Some(&(i, ch)) if ch == '+' || ch == '#' => {
                    sym_end = i + 1;
                    look.next();
                    count += 1;
                }
                _ => break,
            }
        }
        prev_alnum = true;
        if count > 0 && look.peek().is_none_or(|(_, n)| !n.is_alphanumeric()) {
            end = sym_end;
            it = look;
            prev_alnum = false;
        }
        out.push((start, end));
    }
    out
}

fn words(s: &str) -> Vec<&str> {
    word_spans(s).into_iter().map(|(a, b)| &s[a..b]).collect()
}

fn phrase_words(s: &str) -> Vec<String> {
    words(&fold(s)).into_iter().map(str::to_string).collect()
}

// ---------- índice del catálogo ----------

struct Phrase {
    words: Vec<String>,
    entry: usize,
    alias: bool,
}

type Index = HashMap<String, Vec<Phrase>>;

fn add_phrase(index: &mut Index, text: &str, entry: usize, alias: bool) {
    let words = phrase_words(text);
    if words.is_empty() {
        return;
    }
    let list = index.entry(words[0].clone()).or_default();
    if list.iter().any(|p| p.words == words && p.entry == entry) {
        return;
    }
    list.push(Phrase {
        words,
        entry,
        alias,
    });
}

/// Todas las apariciones de frases del índice en una línea: (posición, largo, frase).
fn find_phrases<'a>(index: &'a Index, toks: &[&str]) -> Vec<(usize, usize, &'a Phrase)> {
    let mut out = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let Some(cands) = index.get(*t) else { continue };
        for p in cands {
            let len = p.words.len();
            if i + len <= toks.len() && p.words.iter().zip(&toks[i..]).all(|(a, b)| a == b) {
                out.push((i, len, p));
            }
        }
    }
    out
}

/// Nombres de idioma en otras lenguas, por código (el catálogo solo trae uno).
fn extra_language_names(code: &str) -> &'static [&'static str] {
    match fold(code).as_str() {
        "en" => &["english", "ingles"],
        "es" => &["spanish", "espanol", "castellano"],
        "fr" => &["french", "frances"],
        "de" => &["german", "aleman", "deutsch"],
        "pt" => &["portuguese", "portugues"],
        "it" => &["italian", "italiano"],
        "ca" => &["catalan", "catala"],
        "zh" => &["chinese", "chino", "mandarin"],
        "ja" => &["japanese", "japones"],
        "ko" => &["korean", "coreano"],
        "ru" => &["russian", "ruso"],
        "ar" => &["arabic", "arabe"],
        "nl" => &["dutch", "neerlandes", "holandes"],
        _ => &[],
    }
}

fn proficiency_of(w: &str) -> Option<Proficiency> {
    const NATIVE: &[&str] = &["nativ", "materna", "bilingue", "bilingual"];
    const ADVANCED: &[&str] = &["avanzad", "advanced", "fluid", "fluent", "proficient"];
    const INTERMEDIATE: &[&str] = &["intermedi", "conversacional"];
    const BASIC: &[&str] = &["basic", "elemental", "principiante", "beginner"];
    let any = |stems: &[&str]| stems.iter().any(|s| w.starts_with(s));
    if any(NATIVE) {
        Some(Proficiency::Native)
    } else if any(ADVANCED) || w == "c1" || w == "c2" {
        Some(Proficiency::Advanced)
    } else if any(INTERMEDIATE) || w == "b1" || w == "b2" {
        Some(Proficiency::Intermediate)
    } else if any(BASIC) || w == "a1" || w == "a2" {
        Some(Proficiency::Basic)
    } else {
        None
    }
}

// ---------- depuración de datos de contacto ----------

fn is_email_or_url(w: &str) -> bool {
    const HOSTS: &[&str] = &[
        "linkedin.",
        "github.",
        "gitlab.",
        "bitbucket.",
        "twitter.",
        "facebook.",
        "instagram.",
        "behance.",
        "wa.me",
        "t.me/",
    ];
    // Camino rápido: sin ninguno de estos caracteres no puede ser email ni URL.
    if !w.bytes().any(|b| matches!(b, b'@' | b'/' | b'.')) {
        return false;
    }
    let lw = w.to_ascii_lowercase();
    if lw.contains('@') || lw.contains("://") || lw.starts_with("www.") {
        return true;
    }
    if HOSTS.iter().any(|h| lw.contains(h)) {
        return true;
    }
    // `dominio.tld/ruta`
    matches!(lw.find('/'), Some(p) if p >= 3 && lw[..p].contains('.'))
}

fn is_phoneish(w: &str) -> bool {
    let mut digit = false;
    for b in w.bytes() {
        if b.is_ascii_digit() {
            digit = true;
        } else if !matches!(b, b'+' | b'(' | b')' | b'-' | b'.') {
            return false;
        }
    }
    digit
}

fn starts_phone_run(w: &str) -> bool {
    w.starts_with('+') || w.starts_with('(')
}

fn is_year(g: &str) -> bool {
    g.len() == 4 && matches!(g.parse::<u32>(), Ok(1950..=2100))
}

fn is_phone_run(run: &[&str]) -> bool {
    let mut digits = 0usize;
    for w in run {
        for b in w.bytes() {
            digits += usize::from(b.is_ascii_digit());
        }
        if digits >= 9 {
            break;
        }
    }
    if digits < 8 {
        return false;
    }
    // «2019 2020 2021»: años seguidos, no un teléfono.
    let all_years = run
        .iter()
        .flat_map(|w| w.split(|c: char| !c.is_ascii_digit()))
        .filter(|g| !g.is_empty())
        .all(is_year);
    if all_years {
        return false;
    }
    digits >= 9 || run[0].starts_with('+')
}

/// Quita emails, URLs y teléfonos de una línea. Devuelve la línea limpia, si se quitó algo y
/// cuántas palabras quedan.
fn redact_line(line: &str) -> (String, bool, usize) {
    let ws: Vec<&str> = line.split_whitespace().collect();
    let mut drop = vec![false; ws.len()];
    for (i, w) in ws.iter().enumerate() {
        drop[i] = is_email_or_url(w);
    }
    let mut i = 0;
    while i < ws.len() {
        if drop[i] || !is_phoneish(ws[i]) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < ws.len() && !drop[j] && is_phoneish(ws[j]) && !starts_phone_run(ws[j]) {
            j += 1;
        }
        if is_phone_run(&ws[i..j]) {
            drop[i..j].iter_mut().for_each(|d| *d = true);
        }
        i = j;
    }
    let kept: Vec<&str> = ws
        .iter()
        .zip(&drop)
        .filter(|(_, d)| !**d)
        .map(|(w, _)| *w)
        .collect();
    // Solo cuentan las palabras con letras o números: «-» y «/» sueltos no son contenido.
    let content = kept
        .iter()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count();
    (kept.join(" "), drop.iter().any(|d| *d), content)
}

const STREET_WORDS: &[&str] = &[
    "calle",
    "avenida",
    "av",
    "avda",
    "carrera",
    "cra",
    "street",
    "avenue",
    "ave",
    "road",
    "paseo",
    "pasaje",
    "camino",
    "boulevard",
    "blvd",
    "jiron",
    "rua",
];
const ADDRESS_LABELS: &[&str] = &["direccion", "address", "domicilio", "residencia"];

/// ¿La línea es una dirección? Etiqueta («Dirección: …») o palabra de calle con un número.
fn looks_like_address(text: &str, folded: &str, toks: &[&str]) -> bool {
    let Some(first) = toks.first() else {
        return false;
    };
    if ADDRESS_LABELS.contains(first)
        && folded.trim_start().as_bytes().get(first.len()) == Some(&b':')
    {
        return true;
    }
    if toks.len() > 14 {
        return false;
    }
    let has_street = toks.iter().any(|t| STREET_WORDS.contains(t));
    let has_number = toks.iter().any(|t| t.bytes().any(|b| b.is_ascii_digit()));
    // Un cargo con fechas («… Avenida Corp 2021 – 2023») no es una dirección.
    has_street && has_number && find_range(text).is_none()
}

// ---------- documento por líneas y secciones ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Preamble,
    Profile,
    Experience,
    /// Intereses y referencias: aquí ninguna palabra es una skill de la persona.
    NoSkills,
    Other,
}

impl Line {
    fn toks(&self) -> Vec<&str> {
        self.spans
            .iter()
            .map(|&(a, b)| &self.folded[a..b])
            .collect()
    }
}

struct Line {
    text: String,
    folded: String,
    /// Palabras de `folded` como rangos de bytes (se calculan una sola vez por línea).
    spans: Vec<(usize, usize)>,
    section: Section,
    heading: bool,
}

fn heading_section(ws: &[&str]) -> Option<Section> {
    const PROFILE: &[&str] = &[
        "perfil",
        "perfil profesional",
        "resumen",
        "resumen profesional",
        "sobre mi",
        "acerca de mi",
        "objetivo",
        "objetivo profesional",
        "summary",
        "professional summary",
        "profile",
        "about me",
        "about",
        "objective",
    ];
    const EXPERIENCE: &[&str] = &[
        "experiencia",
        "experiencia laboral",
        "experiencia profesional",
        "historial laboral",
        "trayectoria profesional",
        "work experience",
        "professional experience",
        "experience",
        "employment history",
        "work history",
    ];
    const OTHER: &[&str] = &[
        "idiomas",
        "languages",
        "language skills",
        "educacion",
        "formacion",
        "formacion academica",
        "education",
        "habilidades",
        "skills",
        "competencias",
        "tecnologias",
        "technical skills",
        "habilidades tecnicas",
        "certificaciones",
        "certifications",
        "cursos",
        "proyectos",
        "projects",
        "publicaciones",
        "premios",
        "awards",
        "voluntariado",
        "datos personales",
        "contacto",
        "contact",
        "personal information",
        "informacion personal",
    ];
    const NO_SKILLS: &[&str] = &["intereses", "interests", "referencias", "references"];
    if ws.is_empty() || ws.len() > 5 {
        return None;
    }
    let key = ws.join(" ");
    if NO_SKILLS.contains(&key.as_str()) {
        Some(Section::NoSkills)
    } else if PROFILE.contains(&key.as_str()) {
        Some(Section::Profile)
    } else if EXPERIENCE.contains(&key.as_str()) {
        Some(Section::Experience)
    } else if OTHER.contains(&key.as_str()) {
        Some(Section::Other)
    } else {
        None
    }
}

fn clean_lines(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut section = Section::Preamble;
    for raw in text.lines() {
        // Las celdas de una tabla llegan separadas por tabulaciones: se convierten en `|`,
        // el separador que el resto del extractor ya entiende.
        let tabbed;
        let raw = if raw.contains('\t') {
            tabbed = raw.replace('\t', " | ");
            tabbed.as_str()
        } else {
            raw
        };
        let (clean, redacted, remaining) = redact_line(raw);
        if clean.is_empty() || (redacted && remaining <= 2) {
            continue;
        }
        let folded = fold(&clean);
        let spans = word_spans(&folded);
        let toks: Vec<&str> = spans.iter().map(|&(a, b)| &folded[a..b]).collect();
        if looks_like_address(&clean, &folded, &toks) {
            continue;
        }
        let heading = heading_section(&toks);
        drop(toks);
        if let Some(s) = heading {
            section = s;
        }
        out.push(Line {
            text: clean,
            folded,
            spans,
            section,
            heading: heading.is_some(),
        });
    }
    out
}

fn cut_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.trim().to_string();
    }
    let cut: String = s.chars().take(max).collect();
    // Preferir terminar en una palabra completa si hay un espacio razonablemente cerca.
    let at_boundary = s.chars().nth(max).is_some_and(char::is_whitespace);
    let res = match cut.rfind(' ') {
        Some(p) if !at_boundary && p > max * 3 / 4 => cut[..p].to_string(),
        _ => cut,
    };
    res.trim().to_string()
}

fn strip_bullet(s: &str) -> &str {
    s.trim_start_matches(['-', '•', '*', '·', '▪', '–', '—', '●'])
        .trim()
}

fn is_bullet(s: &str) -> bool {
    s.starts_with(['-', '•', '*', '·', '▪', '●'])
}

// ---------- fechas ----------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Date {
    year: u32,
    month: Option<u32>,
}

struct Range {
    start: Date,
    end: Option<Date>,
    /// Posición (en bytes) del rango dentro de la línea.
    span: (usize, usize),
}

struct DTok {
    folded: String,
    start: usize,
    end: usize,
}

fn date_tokens(line: &str) -> Vec<DTok> {
    let mut out = Vec::new();
    let mut run_start: Option<usize> = None;
    let flush = |out: &mut Vec<DTok>, from: usize, to: usize| {
        out.push(DTok {
            folded: fold(&line[from..to]),
            start: from,
            end: to,
        });
    };
    for (i, c) in line.char_indices() {
        if c.is_alphanumeric() {
            run_start.get_or_insert(i);
            continue;
        }
        if let Some(s) = run_start.take() {
            flush(&mut out, s, i);
        }
        if !c.is_whitespace() {
            flush(&mut out, i, i + c.len_utf8());
        }
    }
    if let Some(s) = run_start {
        flush(&mut out, s, line.len());
    }
    out
}

fn month_of(w: &str) -> Option<u32> {
    Some(match w {
        "ene" | "enero" | "jan" | "january" => 1,
        "feb" | "febrero" | "february" => 2,
        "mar" | "marzo" | "march" => 3,
        "abr" | "abril" | "apr" | "april" => 4,
        "may" | "mayo" => 5,
        "jun" | "junio" | "june" => 6,
        "jul" | "julio" | "july" => 7,
        "ago" | "agosto" | "aug" | "august" => 8,
        "sep" | "sept" | "septiembre" | "setiembre" | "september" => 9,
        "oct" | "octubre" | "october" => 10,
        "nov" | "noviembre" | "november" => 11,
        "dic" | "diciembre" | "dec" | "december" => 12,
        _ => return None,
    })
}

fn year_of(w: &str) -> Option<u32> {
    if is_year(w) {
        w.parse().ok()
    } else {
        None
    }
}

fn is_current(w: &str) -> bool {
    matches!(
        w,
        "actual"
            | "actualidad"
            | "actualmente"
            | "presente"
            | "hoy"
            | "present"
            | "current"
            | "now"
            | "today"
            | "ahora"
    )
}

fn is_range_sep(w: &str) -> bool {
    matches!(
        w,
        "-" | "–"
            | "—"
            | "−"
            | "~"
            | "|"
            | "a"
            | "al"
            | "to"
            | "hasta"
            | "until"
            | "through"
            | "thru"
    )
}

/// Una fecha desde `i`: `mes [de] año`, `MM/año` o `año`. Devuelve la fecha y el siguiente token.
fn parse_date(t: &[DTok], i: usize) -> Option<(Date, usize)> {
    let w = t.get(i)?.folded.as_str();
    if let Some(m) = month_of(w) {
        let mut j = i + 1;
        if t.get(j).is_some_and(|x| x.folded == ".") {
            j += 1;
        }
        if t.get(j)
            .is_some_and(|x| x.folded == "de" || x.folded == "del" || x.folded == ",")
        {
            j += 1;
        }
        if let Some(y) = t.get(j).and_then(|x| year_of(&x.folded)) {
            return Some((
                Date {
                    year: y,
                    month: Some(m),
                },
                j + 1,
            ));
        }
        return None;
    }
    if w.len() <= 2 {
        if let Ok(m @ 1..=12) = w.parse::<u32>() {
            if t.get(i + 1)
                .is_some_and(|x| matches!(x.folded.as_str(), "/" | "-" | "."))
            {
                if let Some(y) = t.get(i + 2).and_then(|x| year_of(&x.folded)) {
                    return Some((
                        Date {
                            year: y,
                            month: Some(m),
                        },
                        i + 3,
                    ));
                }
            }
        }
        return None;
    }
    year_of(w).map(|y| {
        (
            Date {
                year: y,
                month: None,
            },
            i + 1,
        )
    })
}

/// Las fechas de un encabezado están al principio de la línea: no se escanea más allá.
const DATE_SCAN_BYTES: usize = 512;

fn find_range(line: &str) -> Option<Range> {
    let mut cut = line.len().min(DATE_SCAN_BYTES);
    while !line.is_char_boundary(cut) {
        cut -= 1;
    }
    let t = date_tokens(&line[..cut]);
    for i in 0..t.len() {
        let Some((start, mut j)) = parse_date(&t, i) else {
            continue;
        };
        let sep_from = j;
        while j < t.len() && j - sep_from < 3 && is_range_sep(&t[j].folded) {
            j += 1;
        }
        if j == sep_from {
            continue;
        }
        let (end, next) = if t.get(j).is_some_and(|x| is_current(&x.folded)) {
            (None, j + 1)
        } else if let Some((e, n)) = parse_date(&t, j) {
            (Some(e), n)
        } else {
            continue;
        };
        if let Some(e) = end {
            let s_cmp = (start.year, start.month.unwrap_or(1));
            let e_cmp = (e.year, e.month.unwrap_or(12));
            if e_cmp < s_cmp {
                continue;
            }
        }
        return Some(Range {
            start,
            end,
            span: (t[i].start, t[next - 1].end),
        });
    }
    None
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn start_string(d: Date) -> String {
    format!("{:04}-{:02}-01", d.year, d.month.unwrap_or(1))
}

fn end_string(d: Date) -> String {
    let m = d.month.unwrap_or(12);
    format!("{:04}-{:02}-{:02}", d.year, m, days_in_month(d.year, m))
}

// ---------- experiencia ----------

fn has_title_word(folded: &str) -> bool {
    const EXACT: &[&str] = &[
        "qa",
        "sr",
        "jr",
        "dev",
        "sre",
        "head",
        "full",
        "back",
        "front",
        "owner",
        "lead",
        "intern",
        "interns",
        "internship",
        "cto",
        "ceo",
        "pm",
    ];
    const STEMS: &[&str] = &[
        "develop",
        "desarroll",
        "engineer",
        "ingenier",
        "analyst",
        "analis",
        "manager",
        "gerente",
        "lider",
        "director",
        "designer",
        "disenad",
        "consult",
        "architect",
        "arquitect",
        "devops",
        "practicante",
        "becario",
        "scientist",
        "cientific",
        "administrad",
        "programad",
        "programmer",
        "coordinad",
        "jefe",
        "specialist",
        "especialist",
        "senior",
        "junior",
        "fullstack",
        "backend",
        "frontend",
        "tecnico",
        "technician",
        "supervisor",
        "asistente",
        "assistant",
        "trainee",
        "pasante",
        "freelance",
        "profesor",
        "teacher",
        "docente",
        "responsable",
        "encargad",
        "tester",
        "officer",
    ];
    words(folded)
        .iter()
        .any(|w| EXACT.contains(w) || STEMS.iter().any(|s| w.starts_with(s)))
}

fn trim_header(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || "|-–—·•,:;()[]/".contains(c))
}

/// Separa `a SEP b`. Devuelve las dos partes y si el separador fija el orden cargo-empresa.
fn split_header(h: &str) -> Option<(String, String, bool)> {
    let lower = h.to_ascii_lowercase();
    let ordered: &[(&str, bool)] = &[
        ("|", false),
        (" @ ", true),
        (" at ", true),
        (" en ", true),
        (" — ", false),
        (" – ", false),
        (" - ", false),
        (" · ", false),
        (", ", false),
    ];
    for (sep, title_first) in ordered {
        // `en`/`at`/`@`: el último, para no partir «Ingeniero en Sistemas en Acme».
        let pos = if *title_first {
            lower.rfind(sep)
        } else {
            lower.find(sep)
        };
        let Some(p) = pos else { continue };
        let left = trim_header(&h[..p]);
        let right = trim_header(&h[p + sep.len()..]);
        if *sep == "|" {
            let parts: Vec<&str> = h
                .split('|')
                .map(trim_header)
                .filter(|x| !x.is_empty())
                .collect();
            if parts.len() >= 2 {
                return Some((parts[0].to_string(), parts[1].to_string(), false));
            }
            continue;
        }
        if !left.is_empty() && !right.is_empty() {
            return Some((left.to_string(), right.to_string(), *title_first));
        }
    }
    None
}

struct Header {
    company: String,
    title: String,
    certain: bool,
    /// Cuántas líneas anteriores a la de fechas se usaron como encabezado.
    lines_used: usize,
}

fn eligible_neighbor(line: &Line, scope: &dyn Fn(&Line) -> bool) -> bool {
    scope(line)
        && !line.heading
        && !is_bullet(&line.text)
        && line.text.chars().count() <= 60
        && find_range(&line.text).is_none()
}

fn build_header(
    remainder: &str,
    prev1: Option<&Line>,
    prev2: Option<&Line>,
    scope: &dyn Fn(&Line) -> bool,
) -> Option<Header> {
    let (text, mut used) = {
        let r = trim_header(remainder);
        if !r.is_empty() {
            (r.to_string(), 0)
        } else {
            let p = prev1.filter(|l| eligible_neighbor(l, scope))?;
            (trim_header(&p.text).to_string(), 1)
        }
    };
    if text.is_empty() || text.chars().count() > MAX_HEADER_CHARS {
        return None;
    }
    if let Some((l, r, title_first)) = split_header(&text) {
        let (lk, rk) = (has_title_word(&fold(&l)), has_title_word(&fold(&r)));
        let left_is_title = lk;
        return Some(if title_first {
            Header {
                company: r,
                title: l,
                // «Worked at Acme»: si la izquierda no es un cargo, no se sabe qué es.
                certain: left_is_title,
                lines_used: used,
            }
        } else if lk != rk {
            if lk {
                Header {
                    company: r,
                    title: l,
                    certain: true,
                    lines_used: used,
                }
            } else {
                Header {
                    company: l,
                    title: r,
                    certain: true,
                    lines_used: used,
                }
            }
        } else {
            Header {
                company: l,
                title: r,
                certain: false,
                lines_used: used,
            }
        });
    }
    // Una sola cosa: cargo o empresa. La otra puede estar en la línea de más arriba.
    let is_title = has_title_word(&fold(&text));
    let neighbor = if used == 1 {
        prev2.filter(|l| eligible_neighbor(l, scope))
    } else {
        None
    };
    let neighbor_text = neighbor.map(|l| trim_header(&l.text).to_string());
    let (company, title, certain) = if is_title {
        match neighbor_text.filter(|n| n.split_whitespace().count() <= 6 && !n.ends_with('.')) {
            Some(n) => {
                used += 1;
                (n, text, true)
            }
            None => (String::new(), text, false),
        }
    } else {
        match neighbor_text.filter(|n| has_title_word(&fold(n))) {
            Some(n) => {
                used += 1;
                (text, n, true)
            }
            None => (text, String::new(), false),
        }
    };
    Some(Header {
        company,
        title,
        certain,
        lines_used: used,
    })
}

fn extract_experiences(lines: &[Line]) -> Vec<ExperienceProposal> {
    let has_exp_heading = lines
        .iter()
        .any(|l| l.heading && l.section == Section::Experience);
    let scope = |l: &Line| {
        l.section == Section::Experience || (!has_exp_heading && l.section == Section::Preamble)
    };
    // Anclas: líneas con un rango de fechas.
    struct Anchor {
        idx: usize,
        range: Range,
        header: Header,
    }
    let mut anchors: Vec<Anchor> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if line.heading || !scope(line) {
            continue;
        }
        let Some(range) = find_range(&line.text) else {
            continue;
        };
        let remainder = format!(
            "{} {}",
            &line.text[..range.span.0],
            &line.text[range.span.1..]
        );
        let prev1 = idx.checked_sub(1).map(|i| &lines[i]);
        let prev2 = idx.checked_sub(2).map(|i| &lines[i]);
        let Some(header) = build_header(&remainder, prev1, prev2, &scope) else {
            continue;
        };
        anchors.push(Anchor { idx, range, header });
    }
    let mut out = Vec::new();
    for (k, a) in anchors.iter().enumerate().take(MAX_EXPERIENCES) {
        let end_line = match anchors.get(k + 1) {
            Some(next) => next.idx - next.header.lines_used,
            None => lines.len(),
        };
        let mut desc_parts: Vec<&str> = Vec::new();
        for l in lines.iter().take(end_line).skip(a.idx + 1) {
            if l.heading || !scope(l) {
                break;
            }
            let t = strip_bullet(&l.text);
            if !t.is_empty() {
                desc_parts.push(t);
            }
        }
        let joined = desc_parts.join(" ");
        let description = if joined.is_empty() {
            None
        } else {
            Some(cut_chars(&joined, MAX_EXPERIENCE_DESCRIPTION_CHARS))
        };
        let year_only =
            a.range.start.month.is_none() || a.range.end.is_some_and(|e| e.month.is_none());
        let confidence =
            if !a.header.certain || a.header.company.is_empty() || a.header.title.is_empty() {
                Confidence::Low
            } else if year_only {
                Confidence::Medium
            } else {
                Confidence::High
            };
        out.push(ExperienceProposal {
            company: a.header.company.clone(),
            title: a.header.title.clone(),
            start_date: start_string(a.range.start),
            end_date: a.range.end.map(end_string),
            description,
            confidence,
        });
    }
    out
}

/// Skills cuyo nombre o alias es también una palabra común («go», «next», «rest»…). En prosa
/// no cuentan; ver `ambiguous_is_skill`.
const AMBIGUOUS: &[&str] = &[
    "go", "next", "rest", "nest", "shell", "sh", "c", "ci", "cd", "spring", "swift", "unity",
    "express", "torch", "elastic", "spark",
];

/// ¿La palabra ambigua en `pos` está usada como skill? Sí si la línea parece una lista (dos o
/// más separadores o una etiqueta «Stack: …»), o si está escrita con mayúscula inicial y no abre
/// frase, o es una línea corta con mayúscula. En cualquier otro caso es una palabra corriente.
fn ambiguous_is_skill(line: &Line, original: &[(usize, usize)], pos: usize) -> bool {
    let text = line.text.as_str();
    let separators = text
        .chars()
        .filter(|c| matches!(c, ',' | ';' | '|' | '/' | '·' | '•'))
        .count();
    if separators >= 2 {
        return true;
    }
    if let Some(colon) = text.find(':') {
        if text[..colon].split_whitespace().count() <= 3 {
            return true;
        }
    }
    // Sin el mismo número de palabras no se puede ubicar la original: no se arriesga.
    if original.len() != line.spans.len() {
        return false;
    }
    let (a, b) = original[pos];
    let word = &text[a..b];
    if !word.chars().next().is_some_and(char::is_uppercase) {
        return false;
    }
    if original.len() <= 3 {
        return true;
    }
    let before = text[..a].trim_end_matches(|c: char| c.is_whitespace() || strip_marks(c));
    !(before.is_empty() || before.ends_with(['.', '!', '?']))
}

fn strip_marks(c: char) -> bool {
    matches!(c, '-' | '•' | '*' | '·' | '▪' | '–' | '—' | '●')
}

// ---------- extractor ----------

pub struct DeterministicExtractor {
    skills: Vec<SkillEntry>,
    languages: Vec<LanguageEntry>,
    skill_index: Index,
    language_index: Index,
}

impl DeterministicExtractor {
    /// Indexa el catálogo una sola vez.
    pub fn new(skills: Vec<SkillEntry>, languages: Vec<LanguageEntry>) -> Self {
        let mut skill_index = Index::new();
        for (i, s) in skills.iter().enumerate() {
            add_phrase(&mut skill_index, &s.name, i, false);
            add_phrase(&mut skill_index, &s.code, i, false);
            for a in &s.aliases {
                add_phrase(&mut skill_index, a, i, true);
            }
        }
        // Un orden estable de candidatos para que el resultado no dependa del catálogo.
        for list in skill_index.values_mut() {
            list.sort_by(|a, b| (&a.words, a.entry).cmp(&(&b.words, b.entry)));
        }
        let mut language_index = Index::new();
        for (i, l) in languages.iter().enumerate() {
            add_phrase(&mut language_index, &l.name, i, false);
            for n in extra_language_names(&l.code) {
                add_phrase(&mut language_index, n, i, false);
            }
        }
        Self {
            skills,
            languages,
            skill_index,
            language_index,
        }
    }

    fn skills_in(&self, lines: &[Line]) -> Vec<SkillProposal> {
        let mut found: HashMap<usize, Confidence> = HashMap::new();
        for line in lines {
            if line.section == Section::NoSkills {
                continue;
            }
            let toks = line.toks();
            let mut matches = find_phrases(&self.skill_index, &toks);
            // Una mención contenida en otra más larga («GitHub» en «GitHub Actions») no
            // cuenta aparte. Barrido por posición: no hay comparaciones cuadráticas.
            matches.sort_by_key(|(pos, len, p)| (*pos, std::cmp::Reverse(*len), p.entry));
            let mut max_end_before = 0usize; // fin máximo de menciones que empiezan antes
            let mut group_pos = usize::MAX;
            let mut group_len = 0usize;
            let mut group_max_end = 0usize;
            let mut original: Option<Vec<(usize, usize)>> = None;
            for (pos, len, p) in matches {
                if pos != group_pos {
                    max_end_before = max_end_before.max(group_max_end);
                    group_pos = pos;
                    group_len = len;
                }
                group_max_end = group_max_end.max(pos + len);
                if len < group_len || pos + len <= max_end_before {
                    continue;
                }
                let mut conf = if p.alias || (len == 1 && p.words[0].chars().count() <= 2) {
                    Confidence::Medium
                } else {
                    Confidence::High
                };
                if len == 1 && AMBIGUOUS.contains(&p.words[0].as_str()) {
                    let orig = original.get_or_insert_with(|| word_spans(&line.text));
                    if !ambiguous_is_skill(line, orig, pos) {
                        continue;
                    }
                    conf = conf.min(Confidence::Medium);
                }
                let e = found.entry(p.entry).or_insert(conf);
                *e = (*e).max(conf);
            }
        }
        let mut out: Vec<SkillProposal> = found
            .into_iter()
            .map(|(i, confidence)| SkillProposal {
                skill_id: self.skills[i].id.clone(),
                code: self.skills[i].code.clone(),
                name: self.skills[i].name.clone(),
                confidence,
            })
            .collect();
        out.sort_by(|a, b| {
            (b.confidence, fold(&a.name), &a.skill_id).cmp(&(
                a.confidence,
                fold(&b.name),
                &b.skill_id,
            ))
        });
        out.truncate(MAX_SKILLS);
        out.sort_by(|a, b| (fold(&a.name), &a.skill_id).cmp(&(fold(&b.name), &b.skill_id)));
        out
    }

    fn languages_in(&self, lines: &[Line]) -> Vec<LanguageProposal> {
        // entrada del catálogo -> propuesta; gana la primera mención que trae nivel.
        let mut found: HashMap<usize, (Proficiency, bool, Confidence)> = HashMap::new();
        for line in lines {
            let toks = line.toks();
            let mut mentions = find_phrases(&self.language_index, &toks);
            mentions.sort_by_key(|(pos, len, p)| (*pos, std::cmp::Reverse(*len), p.entry));
            let mut claimed_upto = 0usize; // primer índice libre para un nivel «de antes»
            let mut prev_end = 0usize;
            for (n, (pos, len, p)) in mentions.iter().enumerate() {
                if *pos < prev_end {
                    continue; // solapada con la mención anterior
                }
                let end = pos + len;
                let next_start = mentions
                    .iter()
                    .skip(n + 1)
                    .map(|m| m.0)
                    .find(|s| *s >= end)
                    .unwrap_or(toks.len());
                let after_hi = (end + 4).min(next_start);
                // «fluent in English and basic Portuguese»: un nivel pegado (solo espacios) a la
                // mención siguiente, y no al idioma actual, es el nivel del siguiente.
                let belongs_to_next = |k: usize| {
                    k > end
                        && k + 1 < toks.len()
                        && mentions.iter().any(|m| m.0 == k + 1)
                        && line.folded[line.spans[k].1..line.spans[k + 1].0]
                            .chars()
                            .all(char::is_whitespace)
                };
                let after = (end..after_hi).find_map(|k| {
                    proficiency_of(toks[k])
                        .filter(|_| !belongs_to_next(k))
                        .map(|l| (k, l))
                });
                let level = if let Some((k, l)) = after {
                    claimed_upto = k + 1;
                    Some((l, Confidence::High))
                } else {
                    let lo = pos.saturating_sub(3).max(prev_end).max(claimed_upto);
                    (lo..*pos)
                        .rev()
                        .find_map(|k| proficiency_of(toks[k]))
                        .map(|l| (l, Confidence::Medium))
                };
                prev_end = end;
                let candidate = match level {
                    Some((l, c)) => (l, false, c),
                    None => (Proficiency::Intermediate, true, Confidence::Low),
                };
                match found.get(&p.entry) {
                    Some((_, false, _)) => {}
                    Some((_, true, _)) if candidate.1 => {}
                    _ => {
                        found.insert(p.entry, candidate);
                    }
                }
            }
        }
        let mut out: Vec<LanguageProposal> = found
            .into_iter()
            .map(
                |(i, (proficiency, to_confirm, confidence))| LanguageProposal {
                    language_id: self.languages[i].id.clone(),
                    code: self.languages[i].code.clone(),
                    name: self.languages[i].name.clone(),
                    proficiency,
                    to_confirm,
                    confidence,
                },
            )
            .collect();
        out.sort_by(|a, b| (fold(&a.name), &a.language_id).cmp(&(fold(&b.name), &b.language_id)));
        out.truncate(MAX_LANGUAGES);
        out
    }
}

fn profile_description(lines: &[Line]) -> Option<String> {
    let joined = lines
        .iter()
        .filter(|l| l.section == Section::Profile && !l.heading)
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let d = cut_chars(&joined, MAX_DESCRIPTION_CHARS);
    (!d.is_empty()).then_some(d)
}

impl CvExtractor for DeterministicExtractor {
    fn extract(&self, text: &str) -> Proposal {
        let lines = clean_lines(text);
        Proposal {
            description: profile_description(&lines),
            skills: self.skills_in(&lines),
            languages: self.languages_in(&lines),
            experiences: extract_experiences(&lines),
        }
    }
}

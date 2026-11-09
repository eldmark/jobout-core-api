//! Texto plano de un CV (PDF o DOCX) a partir de sus bytes.
//!
//! - El tipo se decide **solo por contenido** (`detect_kind`): nunca por extensión ni por
//!   `Content-Type`, que ni siquiera entran en la API.
//! - Todo error es un [`TextError`] de dominio; ningún camino hace `panic` hacia el llamador.
//! - Límites (ver [`Limits`]): 15 páginas de PDF, 20 MB descomprimidos de DOCX, 10 s.
//!
//! # Limitación conocida: el hilo de parseo no se puede cancelar
//!
//! El parseo corre en un hilo aparte y el llamador espera con un tope de tiempo. Si el tope
//! vence se devuelve [`TextError::TimedOut`] y el llamador sigue, pero **el hilo no se puede
//! matar** en Rust seguro: si el lector se cuelga de verdad, ese hilo seguirá consumiendo CPU
//! (y memoria) hasta que termine o el proceso muera. El proceso en sí no queda bloqueado. La
//! mitigación es acotar la entrada antes (tamaño del archivo en la ruta, 15 páginas, 20 MB);
//! el aislamiento real exigiría un proceso aparte, fuera del alcance de esta fase.

use std::io::{Cursor, Read};
use std::sync::mpsc;
use std::time::Duration;

use quick_xml::events::Event;
use quick_xml::Reader;
use zip::ZipArchive;

const DOCX_MAIN_PART: &str = "word/document.xml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CvKind {
    Pdf,
    Docx,
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Páginas de PDF que se leen como máximo.
    pub max_pages: usize,
    /// Bytes descomprimidos que puede declarar un DOCX (suma de todas sus entradas).
    pub max_uncompressed_bytes: u64,
    /// Tiempo máximo de la extracción.
    pub timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_pages: 15,
            max_uncompressed_bytes: 20 * 1024 * 1024,
            timeout: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedText {
    pub text: String,
    /// `true` si el PDF tenía más páginas que `Limits::max_pages` y se leyó solo el principio.
    pub truncated: bool,
    /// Páginas totales del PDF (`None` en DOCX, que no tiene páginas fijas).
    pub pages: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextError {
    /// El archivo se leyó bien pero no tiene texto (p. ej. un PDF escaneado).
    NoText,
    /// Ilegible: truncado, dañado o no es lo que dice ser.
    Corrupt,
    /// Un DOCX que descomprime a más de lo permitido (zip bomb).
    TooLarge,
    /// La extracción pasó del tope de tiempo.
    TimedOut,
}

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TextError::NoText => "el archivo no tiene texto",
            TextError::Corrupt => "el archivo está dañado o es ilegible",
            TextError::TooLarge => "el archivo descomprimido es demasiado grande",
            TextError::TimedOut => "se agotó el tiempo de lectura",
        })
    }
}

impl std::error::Error for TextError {}

/// Decide el tipo por los primeros bytes: `%PDF-` o un ZIP que contiene `word/document.xml`.
pub fn detect_kind(bytes: &[u8]) -> Option<CvKind> {
    if bytes.starts_with(b"%PDF-") {
        return Some(CvKind::Pdf);
    }
    if bytes.starts_with(b"PK\x03\x04") {
        let archive = ZipArchive::new(Cursor::new(bytes)).ok()?;
        if archive.file_names().any(|n| n == DOCX_MAIN_PART) {
            return Some(CvKind::Docx);
        }
    }
    None
}

/// Extrae el texto. Corre en un hilo con tope de tiempo (ver la nota del módulo).
pub fn extract_text(
    bytes: &[u8],
    kind: CvKind,
    limits: &Limits,
) -> Result<ExtractedText, TextError> {
    let data = bytes.to_vec();
    let limits_copy = *limits;
    run_with_timeout(limits.timeout, move || match kind {
        CvKind::Pdf => pdf_text(&data, limits_copy.max_pages),
        CvKind::Docx => docx_text(&data, limits_copy.max_uncompressed_bytes),
    })?
}

/// Ejecuta `f` en un hilo y espera su resultado hasta `timeout`. Un `panic` dentro del hilo
/// se convierte en `Corrupt`. Si vence el tiempo el hilo queda corriendo (no se puede cancelar).
pub fn run_with_timeout<T, F>(timeout: Duration, f: F) -> Result<T, TextError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("cv-text".into())
        .spawn(move || {
            // Si el llamador ya se fue, el envío falla y no importa.
            let _ = tx.send(f());
        })
        .map_err(|_| TextError::Corrupt)?;
    match rx.recv_timeout(timeout) {
        Ok(v) => Ok(v),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(TextError::TimedOut),
        // El emisor se soltó sin enviar: el hilo hizo panic.
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(TextError::Corrupt),
    }
}

fn non_empty(
    text: String,
    truncated: bool,
    pages: Option<usize>,
) -> Result<ExtractedText, TextError> {
    if text.trim().is_empty() {
        return Err(TextError::NoText);
    }
    Ok(ExtractedText {
        text,
        truncated,
        pages,
    })
}

fn pdf_text(bytes: &[u8], max_pages: usize) -> Result<ExtractedText, TextError> {
    let doc = lopdf::Document::load_mem(bytes).map_err(|_| TextError::Corrupt)?;
    if doc.is_encrypted() {
        return Err(TextError::Corrupt);
    }
    let numbers: Vec<u32> = doc.get_pages().keys().copied().collect();
    if numbers.is_empty() {
        return Err(TextError::Corrupt);
    }
    let total = numbers.len();
    let mut text = String::new();
    let mut failed = 0usize;
    let mut read = 0usize;
    for n in numbers.iter().take(max_pages) {
        read += 1;
        match page_text(&doc, *n) {
            Ok(t) => {
                text.push_str(&t);
                if !text.ends_with('\n') {
                    text.push('\n');
                }
            }
            Err(_) => failed += 1,
        }
    }
    // Si ninguna página se pudo leer, el archivo está dañado; no es «sin texto».
    if failed == read {
        return Err(TextError::Corrupt);
    }
    non_empty(text, total > max_pages, Some(total))
}

/// Texto de una página con saltos de línea en cada desplazamiento vertical (`Td`, `TD`, `T*`,
/// `Tm`, `'`, `"`) y no solo al final del bloque `BT … ET`: `lopdf::extract_text` pega en una
/// línea todo un bloque, y muchos generadores de PDF escriben la página entera en uno.
fn page_text(doc: &lopdf::Document, number: u32) -> Result<String, lopdf::Error> {
    use lopdf::{Encoding, Object};
    fn push_operands(out: &mut String, enc: &Encoding, ops: &[Object]) {
        for o in ops {
            match o {
                Object::String(bytes, _) => {
                    if let Ok(t) = lopdf::Document::decode_text(enc, bytes) {
                        out.push_str(&t);
                    }
                }
                Object::Array(a) => {
                    push_operands(out, enc, a);
                }
                Object::Integer(i) if *i < -100 => out.push(' '),
                Object::Real(r) if *r < -100.0 => out.push(' '),
                _ => {}
            }
        }
    }
    fn newline(out: &mut String) {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
    }
    let page_id = *doc
        .get_pages()
        .get(&number)
        .ok_or(lopdf::Error::PageNumberNotFound(number))?;
    let encodings: std::collections::BTreeMap<Vec<u8>, Encoding> = doc
        .get_page_fonts(page_id)?
        .into_iter()
        .map(|(name, font)| font.get_font_encoding(doc).map(|e| (name, e)))
        .collect::<Result<_, _>>()?;
    let content = lopdf::content::Content::decode(&doc.get_page_content(page_id)?)?;
    let mut out = String::new();
    let mut current: Option<&Encoding> = None;
    let num = |o: Option<&Object>| match o {
        Some(Object::Integer(i)) => *i as f64,
        Some(Object::Real(r)) => f64::from(*r),
        _ => 0.0,
    };
    for op in &content.operations {
        match op.operator.as_str() {
            "Tf" => {
                let name = op.operands.first().and_then(|o| o.as_name().ok());
                current = name.and_then(|n| encodings.get(n));
            }
            "Tj" | "TJ" => {
                if let Some(enc) = current {
                    push_operands(&mut out, enc, &op.operands);
                    if matches!(op.operands.first(), Some(Object::Array(_))) {
                        out.push(' ');
                    }
                }
            }
            "'" | "\"" => {
                newline(&mut out);
                if let Some(enc) = current {
                    push_operands(&mut out, enc, &op.operands);
                }
            }
            "T*" => newline(&mut out),
            "Td" | "TD" if num(op.operands.get(1)) != 0.0 => newline(&mut out),
            "Tm" if num(op.operands.get(5)) != 0.0 => newline(&mut out),
            "ET" => newline(&mut out),
            _ => {}
        }
    }
    Ok(out)
}

fn docx_text(bytes: &[u8], max_bytes: u64) -> Result<ExtractedText, TextError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| TextError::Corrupt)?;
    // 1) Tamaños declarados de todas las entradas, sin descomprimir nada.
    let mut declared: u64 = 0;
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i).map_err(|_| TextError::Corrupt)?;
        declared = declared.saturating_add(entry.size());
    }
    if declared > max_bytes {
        return Err(TextError::TooLarge);
    }
    // 2) Lectura real acotada: el encabezado puede mentir.
    let xml = {
        let entry = archive
            .by_name(DOCX_MAIN_PART)
            .map_err(|_| TextError::Corrupt)?;
        let mut buf = Vec::new();
        entry
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut buf)
            .map_err(|_| TextError::Corrupt)?;
        buf
    };
    if xml.len() as u64 > max_bytes {
        return Err(TextError::TooLarge);
    }
    non_empty(xml_to_text(&xml)?, false, None)
}

/// Párrafos (`w:p`) a líneas, `w:tab` a tabulación, `w:br` a salto; solo el texto de `w:t`.
fn xml_to_text(xml: &[u8]) -> Result<String, TextError> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut out = String::new();
    let mut in_t = false;
    let mut para_has_content = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Err(_) => return Err(TextError::Corrupt),
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"w:t" => in_t = true,
                // Un párrafo anidado (p. ej. en un cuadro de texto) empieza en línea nueva.
                b"w:p" if para_has_content => {
                    out.push('\n');
                    para_has_content = false;
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                b"w:tab" => {
                    out.push('\t');
                    para_has_content = true;
                }
                b"w:br" | b"w:cr" => {
                    out.push('\n');
                    para_has_content = false;
                }
                _ => {}
            },
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"w:t" => in_t = false,
                b"w:p" => {
                    out.push('\n');
                    para_has_content = false;
                }
                _ => {}
            },
            Ok(Event::Text(t)) if in_t => {
                let s = t.unescape().map_err(|_| TextError::Corrupt)?;
                out.push_str(&s);
                para_has_content = true;
            }
            Ok(_) => {}
        }
        buf.clear();
    }
    while out.ends_with('\n') {
        out.pop();
    }
    Ok(out)
}

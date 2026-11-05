//! Casos de verdad de la lectura de texto de CVs (X1–X9 de docs/working/fase2-perfil-cv.md).
//!
//! Se escribieron antes que la implementación, en rojo. Los archivos de prueba se generan acá:
//! no hay fixtures binarios en el repositorio.

use std::io::Write;
use std::time::{Duration, Instant};

use jobout_core_api::cv::text::{
    detect_kind, extract_text, run_with_timeout, CvKind, Limits, TextError,
};
use zip::write::SimpleFileOptions;

// ---------- generadores de archivos ----------

/// PDF mínimo y válido (xref correcta). `None` = página sin texto (como un escaneo).
fn pdf(pages: &[Option<&str>]) -> Vec<u8> {
    let n = pages.len();
    let mut objs: Vec<String> = Vec::new();
    objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {} >>",
        kids.join(" "),
        n
    ));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for (i, page) in pages.iter().enumerate() {
        let content_id = 5 + 2 * i;
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 3 0 R >> >> /Contents {content_id} 0 R >>"
        ));
        let stream = match page {
            Some(t) => {
                let esc = t
                    .replace('\\', "\\\\")
                    .replace('(', "\\(")
                    .replace(')', "\\)");
                format!("BT /F1 12 Tf 72 720 Td ({esc}) Tj ET")
            }
            // Solo un rectángulo: hay contenido, pero ningún texto.
            None => "0 0 100 100 re f".to_string(),
        };
        objs.push(format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            stream.len(),
            stream
        ));
    }
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, o).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objs.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxml\n{}\n%%EOF\n",
            objs.len() + 1,
            xref
        )
        .replace("startxml", "startxref")
        .as_bytes(),
    );
    out
}

fn zip_of(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in entries {
        w.start_file(*name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn document_xml(paragraphs: &[&str]) -> String {
    let body: String = paragraphs
        .iter()
        .map(|p| format!("<w:p><w:r><w:t xml:space=\"preserve\">{p}</w:t></w:r></w:p>"))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
         <w:body>{body}</w:body></w:document>"
    )
}

fn docx(paragraphs: &[&str]) -> Vec<u8> {
    docx_from_xml(document_xml(paragraphs).into_bytes())
}

fn docx_from_xml(xml: Vec<u8>) -> Vec<u8> {
    zip_of(&[
        ("[Content_Types].xml", b"<Types/>".to_vec()),
        ("word/document.xml", xml),
    ])
}

fn limits() -> Limits {
    Limits::default()
}

// ---------- X1 / X2: texto ----------

/// X1: un PDF con texto devuelve ese texto.
#[test]
fn x1_pdf_con_texto_devuelve_el_texto() {
    let bytes = pdf(&[Some("Ana Perez - Rust developer")]);
    let r = extract_text(&bytes, CvKind::Pdf, &limits()).unwrap();
    assert!(
        r.text.contains("Ana Perez - Rust developer"),
        "{:?}",
        r.text
    );
    assert!(!r.truncated);
    assert_eq!(r.pages, Some(1));
}

#[test]
fn x1_pdf_de_varias_paginas_las_junta_en_orden() {
    let bytes = pdf(&[Some("primera"), Some("segunda"), Some("tercera")]);
    let r = extract_text(&bytes, CvKind::Pdf, &limits()).unwrap();
    let (a, b, c) = (
        r.text.find("primera").unwrap(),
        r.text.find("segunda").unwrap(),
        r.text.find("tercera").unwrap(),
    );
    assert!(a < b && b < c);
}

/// X2: un DOCX con texto devuelve ese texto, un párrafo por línea.
#[test]
fn x2_docx_con_texto_devuelve_el_texto() {
    let bytes = docx(&["Ana Perez", "Experiencia", "Rust &amp; Docker"]);
    let r = extract_text(&bytes, CvKind::Docx, &limits()).unwrap();
    assert_eq!(
        r.text.lines().collect::<Vec<_>>(),
        ["Ana Perez", "Experiencia", "Rust & Docker"]
    );
    assert!(!r.truncated);
    assert_eq!(r.pages, None);
}

#[test]
fn x2_docx_respeta_tabulaciones_y_saltos_de_linea() {
    let xml = "<w:document xmlns:w=\"x\"><w:body><w:p><w:r><w:t>Acme</w:t><w:tab/><w:t>2021</w:t>\
               <w:br/><w:t>Dev</w:t></w:r></w:p></w:body></w:document>";
    let r = extract_text(&docx_from_xml(xml.into()), CvKind::Docx, &limits()).unwrap();
    assert_eq!(r.text, "Acme\t2021\nDev");
}

#[test]
fn x2_docx_ignora_texto_fuera_de_wt() {
    let xml = "<w:document xmlns:w=\"x\"><w:body><w:p><w:instrText>OCULTO</w:instrText>\
               <w:r><w:t>visible</w:t></w:r></w:p></w:body></w:document>";
    let r = extract_text(&docx_from_xml(xml.into()), CvKind::Docx, &limits()).unwrap();
    assert!(r.text.contains("visible") && !r.text.contains("OCULTO"));
}

// ---------- X3: sin texto ----------

/// X3: un PDF escaneado (sin texto) devuelve `NoText`, no un texto vacío engañoso.
#[test]
fn x3_pdf_sin_texto_devuelve_no_text() {
    let bytes = pdf(&[None, None]);
    assert_eq!(
        extract_text(&bytes, CvKind::Pdf, &limits()),
        Err(TextError::NoText)
    );
}

#[test]
fn x3_pdf_con_solo_espacios_devuelve_no_text() {
    let bytes = pdf(&[Some("   ")]);
    assert_eq!(
        extract_text(&bytes, CvKind::Pdf, &limits()),
        Err(TextError::NoText)
    );
}

#[test]
fn x3_docx_vacio_devuelve_no_text() {
    let bytes = docx(&[""]);
    assert_eq!(
        extract_text(&bytes, CvKind::Docx, &limits()),
        Err(TextError::NoText)
    );
}

// ---------- X4: corrupto, sin panic ----------

/// X4: un PDF truncado da error de dominio.
#[test]
fn x4_pdf_truncado_es_corrupt() {
    let bytes = pdf(&[Some("hola")]);
    let cortado = &bytes[..bytes.len() / 3];
    assert_eq!(
        extract_text(cortado, CvKind::Pdf, &limits()),
        Err(TextError::Corrupt)
    );
}

#[test]
fn x4_basura_con_cabecera_pdf_es_corrupt() {
    let mut b = b"%PDF-1.4\n".to_vec();
    b.extend((0..2000u32).map(|i| (i * 31 % 251) as u8));
    assert_eq!(
        extract_text(&b, CvKind::Pdf, &limits()),
        Err(TextError::Corrupt)
    );
}

#[test]
fn x4_bytes_vacios_son_corrupt() {
    assert_eq!(
        extract_text(&[], CvKind::Pdf, &limits()),
        Err(TextError::Corrupt)
    );
    assert_eq!(
        extract_text(&[], CvKind::Docx, &limits()),
        Err(TextError::Corrupt)
    );
}

#[test]
fn x4_docx_truncado_es_corrupt() {
    let bytes = docx(&["hola"]);
    let cortado = &bytes[..bytes.len() / 2];
    assert_eq!(
        extract_text(cortado, CvKind::Docx, &limits()),
        Err(TextError::Corrupt)
    );
}

#[test]
fn x4_docx_con_xml_mal_formado_es_corrupt() {
    let bytes = docx_from_xml(b"<w:document><w:body><w:p><w:t>hola</w:body>".to_vec());
    assert_eq!(
        extract_text(&bytes, CvKind::Docx, &limits()),
        Err(TextError::Corrupt)
    );
}

#[test]
fn x4_zip_sin_document_xml_es_corrupt() {
    let bytes = zip_of(&[("otra/cosa.txt", b"hola".to_vec())]);
    assert_eq!(
        extract_text(&bytes, CvKind::Docx, &limits()),
        Err(TextError::Corrupt)
    );
}

// ---------- X5: zip bomb ----------

/// X5: un DOCX que descomprime a más de 20 MB se rechaza.
#[test]
fn x5_docx_que_descomprime_a_mas_de_20mb_es_too_large() {
    let relleno = "a".repeat(21 * 1024 * 1024);
    let bytes = docx(&[&relleno]);
    assert!(
        bytes.len() < 1024 * 1024,
        "el zip debe ser chico: {}",
        bytes.len()
    );
    assert_eq!(
        extract_text(&bytes, CvKind::Docx, &limits()),
        Err(TextError::TooLarge)
    );
}

/// El tamaño declarado de TODAS las entradas cuenta, no solo el de `document.xml`: se rechaza
/// sin descomprimir nada.
#[test]
fn x5_suma_de_tamanos_declarados_de_todas_las_entradas() {
    let bytes = zip_of(&[
        ("word/document.xml", document_xml(&["hola"]).into_bytes()),
        ("word/media/bomba.bin", vec![0u8; 21 * 1024 * 1024]),
    ]);
    assert_eq!(
        extract_text(&bytes, CvKind::Docx, &limits()),
        Err(TextError::TooLarge)
    );
}

/// El tope es sobre el total: dos entradas de 600 con un tope de 1000 se rechazan.
#[test]
fn x5_el_tope_se_aplica_a_la_suma() {
    let xml = document_xml(&["hola"]);
    let extra = vec![0u8; 900];
    let bytes = zip_of(&[
        ("word/document.xml", xml.clone().into_bytes()),
        ("word/media/a.bin", extra),
    ]);
    let total = (xml.len() + 900) as u64;
    let justo = Limits {
        max_uncompressed_bytes: total,
        ..limits()
    };
    assert!(extract_text(&bytes, CvKind::Docx, &justo).is_ok());
    let menos = Limits {
        max_uncompressed_bytes: total - 1,
        ..limits()
    };
    assert_eq!(
        extract_text(&bytes, CvKind::Docx, &menos),
        Err(TextError::TooLarge)
    );
}

// ---------- X6: páginas ----------

fn paginas(n: usize) -> Vec<u8> {
    let textos: Vec<String> = (1..=n).map(|i| format!("MARCA{i:03}X")).collect();
    let refs: Vec<Option<&str>> = textos.iter().map(|s| Some(s.as_str())).collect();
    pdf(&refs)
}

/// X6: más de 15 páginas se procesa hasta la 15 y se indica.
#[test]
fn x6_pdf_de_20_paginas_se_corta_en_15_y_lo_dice() {
    let r = extract_text(&paginas(20), CvKind::Pdf, &limits()).unwrap();
    assert!(r.truncated);
    assert_eq!(r.pages, Some(20));
    assert!(r.text.contains("MARCA015X"));
    assert!(!r.text.contains("MARCA016X"));
    assert!(r.text.contains("MARCA001X"));
}

#[test]
fn x6_pdf_de_exactamente_15_paginas_no_se_marca_truncado() {
    let r = extract_text(&paginas(15), CvKind::Pdf, &limits()).unwrap();
    assert!(!r.truncated);
    assert!(r.text.contains("MARCA015X"));
}

#[test]
fn x6_el_tope_de_paginas_es_configurable() {
    let l = Limits {
        max_pages: 2,
        ..limits()
    };
    let r = extract_text(&paginas(3), CvKind::Pdf, &l).unwrap();
    assert!(r.truncated && r.text.contains("MARCA002X") && !r.text.contains("MARCA003X"));
}

// ---------- X7: tiempo ----------

/// X7: el hilo que tarda más que el tope devuelve `TimedOut` sin esperar a que termine.
#[test]
fn x7_un_trabajo_lento_devuelve_timed_out_a_tiempo() {
    let t0 = Instant::now();
    let r = run_with_timeout(Duration::from_millis(50), || {
        std::thread::sleep(Duration::from_secs(3));
        1
    });
    assert_eq!(r, Err(TextError::TimedOut));
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
}

#[test]
fn x7_un_trabajo_rapido_devuelve_su_valor() {
    assert_eq!(run_with_timeout(Duration::from_secs(5), || 7), Ok(7));
}

#[test]
fn x7_un_panic_dentro_del_hilo_es_corrupt() {
    let r: Result<u8, _> = run_with_timeout(Duration::from_secs(5), || panic!("boom"));
    assert_eq!(r, Err(TextError::Corrupt));
}

/// El tope de `Limits` llega de verdad a `extract_text`.
#[test]
fn x7_extract_text_respeta_limits_timeout() {
    let grande: Vec<String> = (0..60_000)
        .map(|i| format!("linea numero {i} con texto"))
        .collect();
    let refs: Vec<&str> = grande.iter().map(String::as_str).collect();
    let bytes = docx(&refs);
    let l = Limits {
        timeout: Duration::from_millis(1),
        ..limits()
    };
    assert_eq!(
        extract_text(&bytes, CvKind::Docx, &l),
        Err(TextError::TimedOut)
    );
    // Con el tope normal, el mismo archivo se lee.
    assert!(extract_text(&bytes, CvKind::Docx, &limits()).is_ok());
}

// ---------- X8 / X9: tipo por contenido ----------

/// X8: el tipo se decide por los primeros bytes, nunca por nombre ni Content-Type
/// (la API ni siquiera los recibe).
#[test]
fn x8_detecta_pdf_por_cabecera() {
    assert_eq!(detect_kind(&pdf(&[Some("x")])), Some(CvKind::Pdf));
    assert_eq!(detect_kind(b"%PDF-1.7\nlo que sea"), Some(CvKind::Pdf));
}

#[test]
fn x8_detecta_docx_por_zip_con_document_xml() {
    assert_eq!(detect_kind(&docx(&["x"])), Some(CvKind::Docx));
}

#[test]
fn x8_un_zip_sin_document_xml_no_es_docx() {
    let xlsx = zip_of(&[("xl/workbook.xml", b"<w/>".to_vec())]);
    assert_eq!(detect_kind(&xlsx), None);
    let casi = zip_of(&[("word/document.xml.bak", b"<w/>".to_vec())]);
    assert_eq!(detect_kind(&casi), None);
}

#[test]
fn x8_cabeceras_incompletas_o_texto_no_son_cv() {
    assert_eq!(detect_kind(b"%PDF"), None);
    assert_eq!(detect_kind(b"PDF-1.4"), None);
    assert_eq!(detect_kind(b" %PDF-1.4"), None);
    assert_eq!(detect_kind(b"PK\x03\x04"), None);
    assert_eq!(detect_kind(b"hola, esto es un CV en texto plano"), None);
    assert_eq!(detect_kind(&[]), None);
}

/// X9: un ejecutable renombrado `cv.pdf` se rechaza (llega como bytes; el nombre no cuenta).
#[test]
fn x9_ejecutable_renombrado_se_rechaza() {
    let mut elf = b"\x7fELF\x02\x01\x01\x00".to_vec();
    elf.extend(vec![0u8; 512]);
    assert_eq!(detect_kind(&elf), None);
    let mut exe = b"MZ\x90\x00".to_vec();
    exe.extend(vec![0u8; 512]);
    assert_eq!(detect_kind(&exe), None);
    // Y si alguien lo fuerza igual como PDF, es ilegible.
    assert_eq!(
        extract_text(&elf, CvKind::Pdf, &limits()),
        Err(TextError::Corrupt)
    );
}

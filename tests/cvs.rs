//! Casos de verdad de las rutas de CV (U1–U14, L1–L10 y S1–S8 de docs/working/fase2-perfil-cv.md).
//! Contra PostgreSQL real; una base por prueba; el almacén es un doble que se puede inspeccionar
//! y forzar a fallar.
//!
//! Se escribieron antes que la implementación, en rojo. Ver CONTRIBUTING.md.

mod common;

use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
    io::Write as _,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use async_trait::async_trait;
use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use common::{contiene, llaves, TestDb};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use jobout_core_api::{
    app_with_state,
    blob::{BlobError, BlobStore},
    crypto::PiiKeys,
    jwt::JwtKeys,
    state::AppState,
};
use sea_orm::ConnectionTrait;
use serde_json::{json, Value};
use tower::ServiceExt;
use zip::write::SimpleFileOptions;

const CLAVE: &str = "una-clave-larga-123";
const CVS: &str = "/api/v1/cvs";
const MB5: usize = 5 * 1024 * 1024;
const MIME_DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

// ------------------------------------------------------------- doble del almacén

/// Almacén en memoria que se puede inspeccionar y forzar a fallar.
#[derive(Default)]
struct Almacen {
    datos: Mutex<BTreeMap<String, Vec<u8>>>,
    falla_put: AtomicBool,
    falla_delete: AtomicBool,
}

impl Almacen {
    fn claves(&self) -> BTreeSet<String> {
        self.datos.lock().unwrap().keys().cloned().collect()
    }
    fn bytes(&self, clave: &str) -> Option<Vec<u8>> {
        self.datos.lock().unwrap().get(clave).cloned()
    }
}

#[async_trait]
impl BlobStore for Almacen {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), BlobError> {
        if self.falla_put.load(Ordering::SeqCst) {
            return Err(BlobError::Io);
        }
        let mut d = self.datos.lock().unwrap();
        if d.contains_key(key) {
            return Err(BlobError::AlreadyExists);
        }
        d.insert(key.to_string(), bytes.to_vec());
        Ok(())
    }
    async fn get(&self, key: &str) -> Result<Vec<u8>, BlobError> {
        self.bytes(key).ok_or(BlobError::NotFound)
    }
    async fn delete(&self, key: &str) -> Result<(), BlobError> {
        if self.falla_delete.load(Ordering::SeqCst) {
            return Err(BlobError::Io);
        }
        self.datos.lock().unwrap().remove(key);
        Ok(())
    }
}

// ------------------------------------------------------------- generadores de archivos

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
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objs.len() + 1,
            xref
        )
        .as_bytes(),
    );
    out
}

fn cv_pdf(texto: &str) -> Vec<u8> {
    pdf(&[Some(texto)])
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

fn docx(paragraphs: &[&str]) -> Vec<u8> {
    let body: String = paragraphs
        .iter()
        .map(|p| format!("<w:p><w:r><w:t xml:space=\"preserve\">{p}</w:t></w:r></w:p>"))
        .collect();
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
         <w:body>{body}</w:body></w:document>"
    );
    zip_of(&[
        ("[Content_Types].xml", b"<Types/>".to_vec()),
        ("word/document.xml", xml.into_bytes()),
    ])
}

/// Un PDF válido relleno hasta `total` bytes (basura después de `%%EOF`).
fn pdf_de_tamano(total: usize) -> Vec<u8> {
    let mut b = cv_pdf("Relleno");
    assert!(b.len() < total);
    b.resize(total, b' ');
    b
}

// ------------------------------------------------------------- la aplicación

fn pii() -> Arc<PiiKeys> {
    Arc::new(
        PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
    )
}

fn jwt() -> Arc<JwtKeys> {
    Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap())
}

fn app(db: &TestDb, almacen: Arc<Almacen>) -> Router {
    app_with_state(AppState {
        db: db.conn.clone(),
        pii: pii(),
        jwt: jwt(),
        blobs: almacen,
    })
}

struct Resp {
    estado: StatusCode,
    cabeceras: HeaderMap,
    bytes: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes).unwrap_or(Value::Null)
    }
    fn texto(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

async fn crudo(
    app: &Router,
    metodo: &str,
    uri: &str,
    token: Option<&str>,
    tipo: Option<&str>,
    cuerpo: Body,
) -> Resp {
    let mut p = Request::builder().method(metodo).uri(uri);
    if let Some(t) = token {
        p = p.header("authorization", format!("Bearer {t}"));
    }
    if let Some(t) = tipo {
        p = p.header("content-type", t);
    }
    let r = app.clone().oneshot(p.body(cuerpo).unwrap()).await.unwrap();
    let estado = r.status();
    let cabeceras = r.headers().clone();
    let bytes = r.into_body().collect().await.unwrap().to_bytes().to_vec();
    Resp {
        estado,
        cabeceras,
        bytes,
    }
}

async fn simple(app: &Router, metodo: &str, uri: &str, token: &str) -> Resp {
    crudo(app, metodo, uri, Some(token), None, Body::empty()).await
}

const LIMITE: &str = "----jobout-prueba-limite";

/// Cuerpo multipart: cada parte es (campo, nombre de archivo, bytes).
fn multipart(partes: &[(&str, Option<&str>, &[u8])]) -> (String, Vec<u8>) {
    let mut b = Vec::new();
    for (campo, archivo, datos) in partes {
        b.extend_from_slice(format!("--{LIMITE}\r\n").as_bytes());
        let mut cd = format!("Content-Disposition: form-data; name=\"{campo}\"");
        if let Some(a) = archivo {
            cd.push_str(&format!("; filename=\"{a}\""));
        }
        b.extend_from_slice(cd.as_bytes());
        // Mentimos a propósito con el Content-Type de la parte.
        b.extend_from_slice(b"\r\nContent-Type: application/pdf\r\n\r\n");
        b.extend_from_slice(datos);
        b.extend_from_slice(b"\r\n");
    }
    b.extend_from_slice(format!("--{LIMITE}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={LIMITE}"), b)
}

async fn subir_crudo(app: &Router, token: Option<&str>, nombre: &str, datos: &[u8]) -> Resp {
    let (tipo, cuerpo) = multipart(&[("file", Some(nombre), datos)]);
    crudo(app, "POST", CVS, token, Some(&tipo), Body::from(cuerpo)).await
}

async fn subir(app: &Router, token: &str, nombre: &str, datos: &[u8]) -> (StatusCode, Value) {
    let r = subir_crudo(app, Some(token), nombre, datos).await;
    (r.estado, r.json())
}

async fn registrar(app: &Router, email: &str) -> String {
    let r = crudo(
        app,
        "POST",
        "/api/v1/auth/register",
        None,
        Some("application/json"),
        Body::from(
            json!({"email": email, "password": CLAVE, "first_name": "Ana", "last_name": "Prueba"})
                .to_string(),
        ),
    )
    .await;
    assert_eq!(r.estado, StatusCode::CREATED, "{}", r.texto());
    r.json()["access_token"].as_str().unwrap().to_string()
}

async fn entorno() -> (TestDb, Router, Arc<Almacen>, String) {
    let db = TestDb::migrated().await;
    let almacen = Arc::new(Almacen::default());
    let app = app(&db, almacen.clone());
    let token = registrar(&app, "ana@correo-prueba.com").await;
    (db, app, almacen, token)
}

fn id_de(cuerpo: &Value) -> String {
    cuerpo["id"].as_str().unwrap().to_string()
}

fn es_clave_opaca(k: &str) -> bool {
    k.len() == 36 && k.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

// ------------------------------------------------------------------ subida (U1–U14)

/// U1: un PDF válido → 201 con el formato del contrato y la extracción propuesta.
#[tokio::test]
async fn u1_subir_un_pdf_valido() {
    let (db, app, _a, token) = entorno().await;
    let texto = "Ana Perez - Desarrolladora. Experiencia con Rust y Python. English advanced.";
    let (estado, c) = subir(&app, &token, "mi-cv.pdf", &cv_pdf(texto)).await;

    assert_eq!(estado, StatusCode::CREATED, "{c}");
    assert_eq!(c["version"], 1);
    assert_eq!(c["is_primary"], true);
    assert_eq!(c["original_filename"], "mi-cv.pdf");
    assert_eq!(c["size_bytes"], cv_pdf(texto).len());
    assert!(c["created_at"].as_str().unwrap().ends_with('Z'));
    assert!(c["id"].as_str().unwrap().len() == 36);
    assert_eq!(c["extraction"]["status"], "ready", "{c}");
    assert_eq!(c["extraction"]["reason"], Value::Null);
    let p = &c["extraction"]["proposal"];
    for campo in ["description", "skills", "languages", "experiences"] {
        assert!(p.get(campo).is_some(), "falta {campo}: {p}");
    }
    let codigos: Vec<&str> = p["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["code"].as_str().unwrap())
        .collect();
    assert!(codigos.contains(&"rust"), "{codigos:?}");
    assert!(p["skills"][0]["skill_id"].as_str().is_some());
    assert!(c.to_string().find("storage_key").is_none());
    assert_eq!(db.count("SELECT count(*) FROM cv_extractions").await, 1);
    db.cleanup().await;
}

/// U1b: un DOCX también, y su tipo sale del contenido.
#[tokio::test]
async fn u1b_subir_un_docx_valido() {
    let (db, app, almacen, token) = entorno().await;
    let (estado, c) = subir(
        &app,
        &token,
        "cv.docx",
        &docx(&["Ana Perez", "Rust y Python"]),
    )
    .await;
    assert_eq!(estado, StatusCode::CREATED, "{c}");
    assert_eq!(c["extraction"]["status"], "ready");
    let mime = db
        .conn
        .query_one(sea_orm::Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT mime_type FROM cvs".to_string(),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index::<String>(0)
        .unwrap();
    assert_eq!(mime, MIME_DOCX);
    assert_eq!(almacen.claves().len(), 1);
    db.cleanup().await;
}

/// U2: clave aleatoria, `storage_key` cifrada, el nombre no forma parte de la clave; el mime sale
/// del contenido.
#[tokio::test]
async fn u2_el_archivo_queda_bajo_una_clave_aleatoria_y_cifrada() {
    let (db, app, almacen, token) = entorno().await;
    let datos = cv_pdf("Rust");
    let (_, c) = subir(&app, &token, "mi-cv-secreto.docx", &datos).await;
    let id = id_de(&c);

    let claves = almacen.claves();
    assert_eq!(claves.len(), 1);
    let clave = claves.iter().next().unwrap();
    assert!(es_clave_opaca(clave), "{clave}");
    assert!(!clave.contains("secreto") && !clave.contains("cv"));
    assert_eq!(almacen.bytes(clave).unwrap(), datos);

    let cifrada = common::bytes_de(
        &db,
        &format!("SELECT storage_key FROM cvs WHERE id = '{id}'"),
    )
    .await;
    assert!(
        !contiene(&cifrada, clave.as_bytes()),
        "storage_key en claro"
    );
    assert_eq!(llaves().decrypt(&cifrada).unwrap(), *clave);

    // El cliente dijo `.docx` y `application/pdf`: manda el contenido.
    let r = simple(&app, "GET", &format!("{CVS}/{id}/file"), &token).await;
    assert_eq!(r.cabeceras["content-type"], "application/pdf");
    db.cleanup().await;
}

/// U3: el segundo CV es la versión 2 y el primario; el anterior sigue guardado.
#[tokio::test]
async fn u3_el_segundo_cv_pasa_a_ser_el_primario() {
    let (db, app, almacen, token) = entorno().await;
    let (_, a) = subir(&app, &token, "a.pdf", &cv_pdf("uno")).await;
    let (_, b) = subir(&app, &token, "b.pdf", &cv_pdf("dos")).await;
    assert_eq!(b["version"], 2);
    assert_eq!(b["is_primary"], true);

    let lista = simple(&app, "GET", CVS, &token).await.json();
    let lista = lista.as_array().unwrap();
    assert_eq!(lista.len(), 2);
    let vieja = lista.iter().find(|x| x["id"] == a["id"]).unwrap();
    assert_eq!(vieja["is_primary"], false);
    assert_eq!(almacen.claves().len(), 2);
    assert_eq!(
        db.count("SELECT count(*) FROM cvs WHERE is_primary").await,
        1
    );
    db.cleanup().await;
}

/// U4: sin token → 401 y no se guarda nada.
#[tokio::test]
async fn u4_sin_token_es_401() {
    let (db, app, almacen, _t) = entorno().await;
    let r = subir_crudo(&app, None, "a.pdf", &cv_pdf("x")).await;
    assert_eq!(r.estado, StatusCode::UNAUTHORIZED);
    let r = subir_crudo(&app, Some("abc.def.ghi"), "a.pdf", &cv_pdf("x")).await;
    assert_eq!(r.estado, StatusCode::UNAUTHORIZED);
    assert!(almacen.claves().is_empty());
    db.cleanup().await;
}

/// U5: más de 5 MB → 413 sin guardar nada; justo 5 MB sí pasa.
#[tokio::test]
async fn u5_mas_de_5_mb_es_413_y_justo_5_mb_pasa() {
    let (db, app, almacen, token) = entorno().await;
    let r = subir_crudo(&app, Some(&token), "gordo.pdf", &pdf_de_tamano(MB5 + 1)).await;
    assert_eq!(r.estado, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(r.json()["error"], "archivo_demasiado_grande");
    assert!(almacen.claves().is_empty());
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 0);

    let r = subir_crudo(&app, Some(&token), "justo.pdf", &pdf_de_tamano(MB5)).await;
    assert_eq!(r.estado, StatusCode::CREATED, "{}", r.texto());
    assert_eq!(almacen.claves().len(), 1);
    db.cleanup().await;
}

/// U6: lo que no es PDF ni DOCX **por contenido** → 415, aunque se llame `cv.pdf` y diga
/// `application/pdf`.
#[tokio::test]
async fn u6_el_tipo_se_decide_por_contenido() {
    let (db, app, almacen, token) = entorno().await;
    let zip_sin_docx = zip_of(&[("hola.txt", b"hola".to_vec())]);
    for (nombre, datos) in [
        ("cv.pdf", b"esto es texto plano, no un pdf".to_vec()),
        ("cv.pdf", b"MZ\x90\x00 ejecutable".to_vec()),
        ("cv.docx", zip_sin_docx),
        ("cv.pdf", Vec::new()),
    ] {
        let r = subir_crudo(&app, Some(&token), nombre, &datos).await;
        assert_eq!(r.estado, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{nombre}");
        assert_eq!(r.json()["error"], "tipo_no_permitido");
    }
    assert!(almacen.claves().is_empty());
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 0);
    db.cleanup().await;
}

/// U7: sin campo `file`, con dos, o sin multipart → 422, sin guardar nada.
#[tokio::test]
async fn u7_el_campo_file_es_obligatorio_y_unico() {
    let (db, app, almacen, token) = entorno().await;
    let pdf = cv_pdf("x");

    let (tipo, cuerpo) = multipart(&[("otro", Some("a.pdf"), &pdf)]);
    let r = crudo(
        &app,
        "POST",
        CVS,
        Some(&token),
        Some(&tipo),
        Body::from(cuerpo),
    )
    .await;
    assert_eq!(r.estado, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r.json()["error"], "datos_invalidos");

    let (tipo, cuerpo) = multipart(&[("file", Some("a.pdf"), &pdf), ("file", Some("b.pdf"), &pdf)]);
    let r = crudo(
        &app,
        "POST",
        CVS,
        Some(&token),
        Some(&tipo),
        Body::from(cuerpo),
    )
    .await;
    assert_eq!(r.estado, StatusCode::UNPROCESSABLE_ENTITY);

    // Un campo extra junto al `file` tampoco es válido: un único campo.
    let (tipo, cuerpo) = multipart(&[("file", Some("a.pdf"), &pdf), ("otro", None, b"x")]);
    let r = crudo(
        &app,
        "POST",
        CVS,
        Some(&token),
        Some(&tipo),
        Body::from(cuerpo),
    )
    .await;
    assert_eq!(r.estado, StatusCode::UNPROCESSABLE_ENTITY);

    // Cuerpo vacío, JSON y multipart roto.
    let r = crudo(&app, "POST", CVS, Some(&token), None, Body::empty()).await;
    assert_eq!(r.estado, StatusCode::UNPROCESSABLE_ENTITY);
    let r = crudo(
        &app,
        "POST",
        CVS,
        Some(&token),
        Some("application/json"),
        Body::from("{}"),
    )
    .await;
    assert_eq!(r.estado, StatusCode::UNPROCESSABLE_ENTITY);
    let r = crudo(
        &app,
        "POST",
        CVS,
        Some(&token),
        Some(&format!("multipart/form-data; boundary={LIMITE}")),
        Body::from(format!(
            "--{LIMITE}\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\nrota"
        )),
    )
    .await;
    assert_eq!(r.estado, StatusCode::UNPROCESSABLE_ENTITY);

    assert!(almacen.claves().is_empty());
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 0);
    db.cleanup().await;
}

/// U8: el nombre se guarda sanitizado, solo para mostrarlo, y no afecta dónde se guarda.
#[tokio::test]
async fn u8_el_nombre_se_sanitiza() {
    let (db, app, almacen, token) = entorno().await;
    let largo = format!("{}.pdf", "a".repeat(500));
    let casos = [
        "../../etc/passwd",
        "..\\..\\windows\\system32\\cv.pdf",
        "a\tb\u{2028}c.pdf",
        "/abs/olute/cv.pdf",
        "....//x.pdf",
        &largo,
    ];
    for nombre in casos {
        let (estado, c) = subir(&app, &token, nombre, &cv_pdf("x")).await;
        assert_eq!(estado, StatusCode::CREATED, "{nombre:?}: {c}");
        let guardado = c["original_filename"].as_str().unwrap();
        assert!(!guardado.is_empty(), "{nombre:?}");
        assert!(guardado.chars().count() <= 120, "{nombre:?}: {guardado:?}");
        assert!(!guardado.contains(".."), "{guardado:?}");
        assert!(
            !guardado.contains('/') && !guardado.contains('\\'),
            "{guardado:?}"
        );
        assert!(
            !guardado.chars().any(|c| c.is_control() || c == '\u{2028}'),
            "{guardado:?}"
        );
        // Lo que muestra la lista es lo mismo que lo guardado.
        let en_db = db
            .conn
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DbBackend::Postgres,
                format!(
                    "SELECT original_filename FROM cvs WHERE id = '{}'",
                    id_de(&c)
                ),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get_by_index::<String>(0)
            .unwrap();
        assert_eq!(en_db, guardado);
        // Se borra para no chocar con el tope de CVs.
        let r = simple(&app, "DELETE", &format!("{CVS}/{}", id_de(&c)), &token).await;
        assert_eq!(r.estado, StatusCode::NO_CONTENT);
    }
    // El archivo `/abs/olute/cv.pdf` conserva su último componente legible.
    let (_, c) = subir(&app, &token, "/abs/olute/cv.pdf", &cv_pdf("x")).await;
    assert_eq!(c["original_filename"], "cv.pdf");
    // Todas las claves son opacas.
    assert!(almacen.claves().iter().all(|k| es_clave_opaca(k)));
    db.cleanup().await;
}

/// U9: el sexto CV → 409 `limite_de_cvs`; al borrar uno se puede volver a subir.
#[tokio::test]
async fn u9_maximo_cinco_cvs() {
    let (db, app, almacen, token) = entorno().await;
    let mut ids = vec![];
    for i in 0..5 {
        let (estado, c) = subir(&app, &token, &format!("{i}.pdf"), &cv_pdf("x")).await;
        assert_eq!(estado, StatusCode::CREATED, "{c}");
        ids.push(id_de(&c));
    }
    let r = subir_crudo(&app, Some(&token), "seis.pdf", &cv_pdf("x")).await;
    assert_eq!(r.estado, StatusCode::CONFLICT);
    assert_eq!(r.json()["error"], "limite_de_cvs");
    assert!(r.json()["message"].as_str().unwrap().len() > 10);
    assert_eq!(almacen.claves().len(), 5, "el rechazado no deja archivo");
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 5);

    let r = simple(&app, "DELETE", &format!("{CVS}/{}", ids[0]), &token).await;
    assert_eq!(r.estado, StatusCode::NO_CONTENT);
    let (estado, c) = subir(&app, &token, "otra.pdf", &cv_pdf("x")).await;
    assert_eq!(estado, StatusCode::CREATED, "{c}");
    db.cleanup().await;
}

/// U9b: el tope es por persona, no global.
#[tokio::test]
async fn u9b_el_tope_es_por_persona() {
    let (db, app, _a, token) = entorno().await;
    let otro = registrar(&app, "luis@correo-prueba.com").await;
    for i in 0..5 {
        subir(&app, &token, &format!("{i}.pdf"), &cv_pdf("x")).await;
    }
    let (estado, c) = subir(&app, &otro, "mio.pdf", &cv_pdf("x")).await;
    assert_eq!(estado, StatusCode::CREATED, "{c}");
    assert_eq!(c["version"], 1);
    assert_eq!(c["is_primary"], true);
    db.cleanup().await;
}

/// U10: si el almacén falla al guardar, no queda fila (todo o nada) y el 500 es genérico.
#[tokio::test]
async fn u10_si_el_almacen_falla_no_queda_fila() {
    let (db, app, almacen, token) = entorno().await;
    almacen.falla_put.store(true, Ordering::SeqCst);
    let r = subir_crudo(&app, Some(&token), "mi-nombre-privado.pdf", &cv_pdf("x")).await;
    assert_eq!(r.estado, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(r.json()["error"], "error_interno");
    assert!(!r.texto().contains("mi-nombre-privado"));
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 0);
    assert_eq!(db.count("SELECT count(*) FROM cv_extractions").await, 0);

    // Y al recuperarse el almacén, la siguiente subida es la versión 1.
    almacen.falla_put.store(false, Ordering::SeqCst);
    let (estado, c) = subir(&app, &token, "a.pdf", &cv_pdf("x")).await;
    assert_eq!(estado, StatusCode::CREATED);
    assert_eq!(c["version"], 1);
    db.cleanup().await;
}

/// U11: si la base falla después de guardar el archivo, el archivo se borra.
#[tokio::test]
async fn u11_si_la_base_falla_el_archivo_se_borra() {
    let (db, app, almacen, token) = entorno().await;
    db.exec("DROP TABLE cv_extractions").await.unwrap();
    let r = subir_crudo(&app, Some(&token), "a.pdf", &cv_pdf("x")).await;
    assert_eq!(r.estado, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(r.json()["error"], "error_interno");
    assert!(almacen.claves().is_empty(), "archivo huérfano");
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 0);
    assert_eq!(
        db.count("SELECT count(*) FROM pending_storage_deletions")
            .await,
        0
    );
    db.cleanup().await;
}

/// U11b: si además el borrado del archivo falla, la clave (cifrada) queda en la bandeja.
#[tokio::test]
async fn u11b_si_tampoco_se_puede_borrar_queda_en_la_bandeja() {
    let (db, app, almacen, token) = entorno().await;
    db.exec("DROP TABLE cv_extractions").await.unwrap();
    almacen.falla_delete.store(true, Ordering::SeqCst);
    let r = subir_crudo(&app, Some(&token), "a.pdf", &cv_pdf("x")).await;
    assert_eq!(r.estado, StatusCode::INTERNAL_SERVER_ERROR);

    let claves = almacen.claves();
    assert_eq!(claves.len(), 1);
    assert_eq!(
        db.count("SELECT count(*) FROM pending_storage_deletions")
            .await,
        1
    );
    let cifrada = common::bytes_de(&db, "SELECT storage_key FROM pending_storage_deletions").await;
    let clave = claves.iter().next().unwrap();
    assert!(!contiene(&cifrada, clave.as_bytes()));
    assert_eq!(llaves().decrypt(&cifrada).unwrap(), *clave);
    db.cleanup().await;
}

/// U12: sin texto (escaneado) → 201 con extracción `failed`; el CV queda guardado.
#[tokio::test]
async fn u12_un_cv_sin_texto_se_guarda_con_extraccion_fallida() {
    let (db, app, almacen, token) = entorno().await;
    let (estado, c) = subir(&app, &token, "escaneo.pdf", &pdf(&[None])).await;
    assert_eq!(estado, StatusCode::CREATED, "{c}");
    assert_eq!(c["is_primary"], true);
    assert_eq!(c["extraction"]["status"], "failed");
    assert_eq!(c["extraction"]["reason"], "sin_texto");
    assert_eq!(c["extraction"]["proposal"], Value::Null);
    assert_eq!(almacen.claves().len(), 1);
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 1);

    let fila = db
        .conn
        .query_one(sea_orm::Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT status::text, reason, model, extracted::text FROM cv_extractions".to_string(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fila.try_get_by_index::<String>(0).unwrap(), "failed");
    assert_eq!(fila.try_get_by_index::<String>(1).unwrap(), "sin_texto");
    assert_eq!(
        fila.try_get_by_index::<String>(2).unwrap(),
        "deterministic-v1"
    );
    assert_eq!(fila.try_get_by_index::<String>(3).unwrap(), "{}");
    db.cleanup().await;
}

/// U12b: un PDF ilegible → `ilegible`, también 201.
#[tokio::test]
async fn u12b_un_pdf_ilegible_se_guarda_con_extraccion_fallida() {
    let (db, app, _a, token) = entorno().await;
    let (estado, c) = subir(&app, &token, "roto.pdf", b"%PDF-1.4\nesto no es un pdf").await;
    assert_eq!(estado, StatusCode::CREATED, "{c}");
    assert_eq!(c["extraction"]["status"], "failed");
    assert_eq!(c["extraction"]["reason"], "ilegible");
    db.cleanup().await;
}

/// U12c: una extracción lista se guarda con el modelo y la propuesta serializada.
#[tokio::test]
async fn u12c_la_extraccion_lista_se_guarda_con_su_propuesta() {
    let (db, app, _a, token) = entorno().await;
    let (_, c) = subir(&app, &token, "a.pdf", &cv_pdf("Experiencia con Rust")).await;
    let fila = db
        .conn
        .query_one(sea_orm::Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT status::text, reason, model, extracted::text, reviewed_by_applicant \
             FROM cv_extractions"
                .to_string(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fila.try_get_by_index::<String>(0).unwrap(), "ready");
    assert_eq!(fila.try_get_by_index::<Option<String>>(1).unwrap(), None);
    assert_eq!(
        fila.try_get_by_index::<String>(2).unwrap(),
        "deterministic-v1"
    );
    let guardado: Value =
        serde_json::from_str(&fila.try_get_by_index::<String>(3).unwrap()).unwrap();
    assert_eq!(guardado, c["extraction"]["proposal"]);
    assert!(!fila.try_get_by_index::<bool>(4).unwrap());
    db.cleanup().await;
}

/// U13: subidas simultáneas del mismo candidato → versiones distintas y un solo primario.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn u13_subidas_simultaneas_dejan_un_solo_primario() {
    let (db, app, almacen, token) = entorno().await;
    let mut tareas = vec![];
    for i in 0..4 {
        let (app, token) = (app.clone(), token.clone());
        tareas.push(tokio::spawn(async move {
            subir(&app, &token, &format!("{i}.pdf"), &cv_pdf("Rust")).await
        }));
    }
    let mut versiones = BTreeSet::new();
    for t in tareas {
        let (estado, c) = t.await.unwrap();
        assert_eq!(estado, StatusCode::CREATED, "{c}");
        versiones.insert(c["version"].as_i64().unwrap());
    }
    assert_eq!(versiones, BTreeSet::from([1, 2, 3, 4]));
    assert_eq!(
        db.count("SELECT count(*) FROM cvs WHERE is_primary").await,
        1
    );
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 4);
    assert_eq!(almacen.claves().len(), 4);
    db.cleanup().await;
}

/// U13b: las subidas simultáneas también respetan el tope de 5.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn u13b_las_subidas_simultaneas_respetan_el_tope() {
    let (db, app, almacen, token) = entorno().await;
    let mut tareas = vec![];
    for i in 0..8 {
        let (app, token) = (app.clone(), token.clone());
        tareas.push(tokio::spawn(async move {
            subir(&app, &token, &format!("{i}.pdf"), &cv_pdf("Rust"))
                .await
                .0
        }));
    }
    let mut estados = vec![];
    for t in tareas {
        estados.push(t.await.unwrap());
    }
    assert_eq!(
        estados
            .iter()
            .filter(|e| **e == StatusCode::CREATED)
            .count(),
        5,
        "{estados:?}"
    );
    assert_eq!(
        estados
            .iter()
            .filter(|e| **e == StatusCode::CONFLICT)
            .count(),
        3,
        "{estados:?}"
    );
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 5);
    assert_eq!(almacen.claves().len(), 5, "los rechazados no dejan archivo");
    db.cleanup().await;
}

// ------------------------------------------------------------------ leer y borrar (L1–L10)

/// L1: la lista trae solo mis CVs, con los campos del contrato y sin `storage_key`.
#[tokio::test]
async fn l1_la_lista_trae_solo_mis_cvs() {
    let (db, app, _a, token) = entorno().await;
    let otro = registrar(&app, "luis@correo-prueba.com").await;
    let (_, mio) = subir(&app, &token, "mio.pdf", &cv_pdf("x")).await;
    subir(&app, &otro, "ajeno.pdf", &cv_pdf("y")).await;

    let r = simple(&app, "GET", CVS, &token).await;
    assert_eq!(r.estado, StatusCode::OK);
    let lista = r.json();
    let lista = lista.as_array().unwrap();
    assert_eq!(lista.len(), 1);
    let fila = lista[0].as_object().unwrap();
    let campos: BTreeSet<&str> = fila.keys().map(|k| k.as_str()).collect();
    assert_eq!(
        campos,
        BTreeSet::from([
            "id",
            "version",
            "is_primary",
            "original_filename",
            "size_bytes",
            "created_at"
        ])
    );
    assert_eq!(fila["id"], mio["id"]);
    assert_eq!(fila["original_filename"], "mio.pdf");
    assert!(!r.texto().contains("ajeno"));
    // Sin CVs: lista vacía, nunca 404.
    let nuevo = registrar(&app, "nuevo@correo-prueba.com").await;
    let r = simple(&app, "GET", CVS, &nuevo).await;
    assert_eq!((r.estado, r.json()), (StatusCode::OK, json!([])));
    db.cleanup().await;
}

/// L1b: la lista va de la versión más nueva a la más vieja.
#[tokio::test]
async fn l1b_la_lista_va_de_la_mas_nueva_a_la_mas_vieja() {
    let (db, app, _a, token) = entorno().await;
    for i in 0..3 {
        subir(&app, &token, &format!("{i}.pdf"), &cv_pdf("x")).await;
    }
    let lista = simple(&app, "GET", CVS, &token).await.json();
    let versiones: Vec<i64> = lista
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["version"].as_i64().unwrap())
        .collect();
    assert_eq!(versiones, vec![3, 2, 1]);
    db.cleanup().await;
}

/// L2: la descarga entrega los mismos bytes, como adjunto y con `nosniff`.
#[tokio::test]
async fn l2_la_descarga_entrega_los_mismos_bytes() {
    let (db, app, _a, token) = entorno().await;
    let datos = cv_pdf("Hola Rust");
    let (_, c) = subir(&app, &token, "Mi CV ñandú.pdf", &datos).await;
    let r = simple(&app, "GET", &format!("{CVS}/{}/file", id_de(&c)), &token).await;
    assert_eq!(r.estado, StatusCode::OK);
    assert_eq!(r.bytes, datos);
    assert_eq!(r.cabeceras["content-type"], "application/pdf");
    assert_eq!(r.cabeceras["x-content-type-options"], "nosniff");
    let cd = r.cabeceras["content-disposition"].to_str().unwrap();
    assert!(cd.starts_with("attachment;"), "{cd}");
    assert!(cd.contains("filename*=UTF-8''"), "{cd}");
    assert!(cd.contains("%C3%B1"), "la ñ va codificada: {cd}");
    assert!(cd.is_ascii());

    let (_, d) = subir(&app, &token, "x.docx", &docx(&["Hola"])).await;
    let r = simple(&app, "GET", &format!("{CVS}/{}/file", id_de(&d)), &token).await;
    assert_eq!(r.cabeceras["content-type"], MIME_DOCX);
    db.cleanup().await;
}

/// L2b: el nombre en `Content-Disposition` no puede inyectar encabezados ni salirse de comillas.
#[tokio::test]
async fn l2b_el_nombre_no_inyecta_encabezados() {
    let (db, app, _a, token) = entorno().await;
    let (_, c) = subir(&app, &token, "a\";x=\"y;\u{2028}b.pdf", &cv_pdf("x")).await;
    let r = simple(&app, "GET", &format!("{CVS}/{}/file", id_de(&c)), &token).await;
    assert_eq!(r.estado, StatusCode::OK);
    let cd = r.cabeceras["content-disposition"].to_str().unwrap();
    assert!(!cd.contains('"') && !cd.contains('\n'), "{cd}");
    db.cleanup().await;
}

/// L3 / S1: el CV de otro usuario por id → 404 en todas las rutas, y no le pasa nada.
#[tokio::test]
async fn l3_el_cv_ajeno_es_404_siempre() {
    let (db, app, almacen, token) = entorno().await;
    let otro = registrar(&app, "luis@correo-prueba.com").await;
    let (_, mio) = subir(&app, &token, "mio.pdf", &cv_pdf("Rust")).await;
    let (_, de_luis) = subir(&app, &otro, "luis.pdf", &cv_pdf("x")).await;
    let ajeno = id_de(&de_luis);

    for (metodo, ruta) in [
        ("GET", format!("{CVS}/{ajeno}/file")),
        ("PUT", format!("{CVS}/{ajeno}/primary")),
        ("DELETE", format!("{CVS}/{ajeno}")),
    ] {
        let r = simple(&app, metodo, &ruta, &token).await;
        assert_eq!(r.estado, StatusCode::NOT_FOUND, "{metodo} {ruta}");
        assert_eq!(r.json()["error"], "no_encontrado");
        assert!(r.bytes.windows(4).all(|w| w != b"%PDF"));
    }
    // Nada cambió para Luis ni para mí.
    let r = simple(&app, "GET", CVS, &otro).await;
    assert_eq!(r.json().as_array().unwrap().len(), 1);
    assert_eq!(r.json()[0]["is_primary"], true);
    assert_eq!(almacen.claves().len(), 2);
    assert_eq!(
        simple(&app, "GET", CVS, &token).await.json()[0]["id"],
        mio["id"]
    );
    db.cleanup().await;
}

/// L4: id mal formado o inexistente → 404 (nunca 400 ni 500).
#[tokio::test]
async fn l4_un_id_invalido_o_inexistente_es_404() {
    let (db, app, _a, token) = entorno().await;
    for id in [
        "no-es-un-uuid",
        "123",
        "00000000-0000-7000-8000-000000000000",
        "%27%3B%20DROP%20TABLE%20cvs%3B--",
        "..%2F..%2Fetc",
        "00000000-0000-7000-8000-00000000000g",
    ] {
        for (metodo, sufijo) in [("GET", "/file"), ("PUT", "/primary"), ("DELETE", "")] {
            let r = simple(&app, metodo, &format!("{CVS}/{id}{sufijo}"), &token).await;
            assert_eq!(r.estado, StatusCode::NOT_FOUND, "{metodo} {id}");
            assert_eq!(r.json()["error"], "no_encontrado");
        }
    }
    db.cleanup().await;
}

/// L5: `PUT /primary` cambia el primario (204); el de otro usuario → 404.
#[tokio::test]
async fn l5_cambiar_el_primario() {
    let (db, app, _a, token) = entorno().await;
    let (_, a) = subir(&app, &token, "a.pdf", &cv_pdf("x")).await;
    let (_, b) = subir(&app, &token, "b.pdf", &cv_pdf("y")).await;
    let r = simple(&app, "PUT", &format!("{CVS}/{}/primary", id_de(&a)), &token).await;
    assert_eq!(r.estado, StatusCode::NO_CONTENT);
    assert!(r.bytes.is_empty());
    let lista = simple(&app, "GET", CVS, &token).await.json();
    for fila in lista.as_array().unwrap() {
        assert_eq!(fila["is_primary"], fila["id"] == a["id"], "{lista}");
    }
    // Repetirlo es inocuo.
    let r = simple(&app, "PUT", &format!("{CVS}/{}/primary", id_de(&a)), &token).await;
    assert_eq!(r.estado, StatusCode::NO_CONTENT);
    assert_eq!(
        db.count("SELECT count(*) FROM cvs WHERE is_primary").await,
        1
    );

    let otro = registrar(&app, "luis@correo-prueba.com").await;
    let r = simple(&app, "PUT", &format!("{CVS}/{}/primary", id_de(&b)), &otro).await;
    assert_eq!(r.estado, StatusCode::NOT_FOUND);
    // Y el primario de Ana no cambió por ese intento.
    let lista = simple(&app, "GET", CVS, &token).await.json();
    let primario = lista
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["is_primary"] == true)
        .unwrap();
    assert_eq!(primario["id"], a["id"]);
    db.cleanup().await;
}

/// L6: borrar → 204; se van la fila, la extracción y el archivo.
#[tokio::test]
async fn l6_borrar_se_lleva_fila_extraccion_y_archivo() {
    let (db, app, almacen, token) = entorno().await;
    let (_, a) = subir(&app, &token, "a.pdf", &cv_pdf("Rust")).await;
    let (_, b) = subir(&app, &token, "b.pdf", &cv_pdf("Rust")).await;
    let clave_a = common::bytes_de(
        &db,
        &format!("SELECT storage_key FROM cvs WHERE id = '{}'", id_de(&a)),
    )
    .await;
    let clave_a = llaves().decrypt(&clave_a).unwrap();

    let r = simple(&app, "DELETE", &format!("{CVS}/{}", id_de(&a)), &token).await;
    assert_eq!(r.estado, StatusCode::NO_CONTENT);
    assert!(r.bytes.is_empty());
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 1);
    assert_eq!(db.count("SELECT count(*) FROM cv_extractions").await, 1);
    assert!(almacen.bytes(&clave_a).is_none(), "el archivo sigue");
    assert_eq!(almacen.claves().len(), 1, "solo se borra el pedido");
    let r = simple(&app, "GET", &format!("{CVS}/{}/file", id_de(&a)), &token).await;
    assert_eq!(r.estado, StatusCode::NOT_FOUND);
    let r = simple(&app, "GET", &format!("{CVS}/{}/file", id_de(&b)), &token).await;
    assert_eq!(r.estado, StatusCode::OK);
    assert_eq!(
        db.count("SELECT count(*) FROM pending_storage_deletions")
            .await,
        0
    );
    db.cleanup().await;
}

/// L7: borrar el primario promueve al más reciente que quede; si era el único, no queda ninguno.
#[tokio::test]
async fn l7_borrar_el_primario_promueve_al_mas_reciente() {
    let (db, app, _a, token) = entorno().await;
    let (_, v1) = subir(&app, &token, "1.pdf", &cv_pdf("x")).await;
    let (_, v2) = subir(&app, &token, "2.pdf", &cv_pdf("x")).await;
    let (_, v3) = subir(&app, &token, "3.pdf", &cv_pdf("x")).await;
    // El primario pasa a ser el más viejo; los otros dos quedan no primarios.
    simple(
        &app,
        "PUT",
        &format!("{CVS}/{}/primary", id_de(&v1)),
        &token,
    )
    .await;
    let r = simple(&app, "DELETE", &format!("{CVS}/{}", id_de(&v1)), &token).await;
    assert_eq!(r.estado, StatusCode::NO_CONTENT);

    let lista = simple(&app, "GET", CVS, &token).await.json();
    let primarios: Vec<&Value> = lista
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["is_primary"] == true)
        .collect();
    assert_eq!(primarios.len(), 1, "{lista}");
    assert_eq!(primarios[0]["id"], v3["id"], "el más reciente, no el v2");

    // Borrar uno que no es primario no mueve el primario.
    simple(&app, "DELETE", &format!("{CVS}/{}", id_de(&v2)), &token).await;
    let lista = simple(&app, "GET", CVS, &token).await.json();
    assert_eq!(lista[0]["id"], v3["id"]);
    assert_eq!(lista[0]["is_primary"], true);

    // El único: no queda ninguno.
    simple(&app, "DELETE", &format!("{CVS}/{}", id_de(&v3)), &token).await;
    assert_eq!(simple(&app, "GET", CVS, &token).await.json(), json!([]));
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 0);
    db.cleanup().await;
}

/// L8: si el almacén falla al borrar, el CV desaparece para la persona y la clave queda cifrada
/// en `pending_storage_deletions`.
#[tokio::test]
async fn l8_si_el_almacen_falla_la_clave_queda_pendiente() {
    let (db, app, almacen, token) = entorno().await;
    let (_, a) = subir(&app, &token, "a.pdf", &cv_pdf("x")).await;
    let clave = almacen.claves().into_iter().next().unwrap();
    almacen.falla_delete.store(true, Ordering::SeqCst);

    let r = simple(&app, "DELETE", &format!("{CVS}/{}", id_de(&a)), &token).await;
    assert_eq!(r.estado, StatusCode::NO_CONTENT);
    assert_eq!(simple(&app, "GET", CVS, &token).await.json(), json!([]));
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 0);
    assert_eq!(
        db.count("SELECT count(*) FROM pending_storage_deletions")
            .await,
        1
    );
    let cifrada = common::bytes_de(&db, "SELECT storage_key FROM pending_storage_deletions").await;
    assert!(!contiene(&cifrada, clave.as_bytes()));
    assert_eq!(llaves().decrypt(&cifrada).unwrap(), clave);
    assert!(
        almacen.bytes(&clave).is_some(),
        "el archivo sigue ahí, pendiente"
    );
    db.cleanup().await;
}

/// L9: el mismo DELETE dos veces → el segundo es 404.
#[tokio::test]
async fn l9_borrar_dos_veces_da_404_la_segunda() {
    let (db, app, _a, token) = entorno().await;
    let (_, a) = subir(&app, &token, "a.pdf", &cv_pdf("x")).await;
    let ruta = format!("{CVS}/{}", id_de(&a));
    assert_eq!(
        simple(&app, "DELETE", &ruta, &token).await.estado,
        StatusCode::NO_CONTENT
    );
    let r = simple(&app, "DELETE", &ruta, &token).await;
    assert_eq!(r.estado, StatusCode::NOT_FOUND);
    assert_eq!(r.json()["error"], "no_encontrado");
    db.cleanup().await;
}

/// Sin token, todas las rutas de CV → 401.
#[tokio::test]
async fn s2_las_rutas_de_cv_exigen_token() {
    let (db, app, _a, token) = entorno().await;
    let (_, a) = subir(&app, &token, "a.pdf", &cv_pdf("x")).await;
    let id = id_de(&a);
    for (metodo, ruta) in [
        ("GET", CVS.to_string()),
        ("GET", format!("{CVS}/{id}/file")),
        ("PUT", format!("{CVS}/{id}/primary")),
        ("DELETE", format!("{CVS}/{id}")),
    ] {
        let r = crudo(&app, metodo, &ruta, None, None, Body::empty()).await;
        assert_eq!(r.estado, StatusCode::UNAUTHORIZED, "{metodo} {ruta}");
    }
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 1);
    db.cleanup().await;
}

// ------------------------------------------------------------------ seguridad (S3, S4, S6, S7, S8, L10)

#[derive(Clone, Default)]
struct Registro(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Registro {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Registro {
    fn texto(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

fn capturar_log() -> (Registro, tracing::subscriber::DefaultGuard) {
    let registro = Registro::default();
    let escritor = registro.clone();
    let guarda = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            // El filtro de producción al máximo (RUST_LOG=trace): es lo que hay que garantizar.
            .with_env_filter(jobout_core_api::telemetry::env_filter(Some("trace")))
            .with_ansi(false)
            .with_writer(move || escritor.clone())
            .finish(),
    );
    (registro, guarda)
}

const NOMBRE_SECRETO: &str = "curriculum-confidencial-de-ana";
const TEXTO_SECRETO: &str = "FRASESECRETADELCONTENIDO";

/// L10 / S3 / S4 / S7: ni el nombre, ni el contenido, ni el texto, ni la clave aparecen en el log
/// ni en las respuestas de error, en las rutas correctas y en las que fallan.
#[tokio::test]
async fn l10_nada_sensible_aparece_en_el_log_ni_en_los_errores() {
    let (log, _guarda) = capturar_log();
    let (db, app, almacen, token) = entorno().await;
    let nombre = format!("{NOMBRE_SECRETO}.pdf");
    let datos = cv_pdf(&format!("Experiencia con Rust. {TEXTO_SECRETO}"));
    let mut respuestas: Vec<String> = vec![];

    let r = subir_crudo(&app, Some(&token), &nombre, &datos).await;
    assert_eq!(r.estado, StatusCode::CREATED);
    let id = id_de(&r.json());
    let clave = almacen.claves().into_iter().next().unwrap();
    simple(&app, "GET", CVS, &token).await;
    simple(&app, "GET", &format!("{CVS}/{id}/file"), &token).await;
    simple(&app, "PUT", &format!("{CVS}/{id}/primary"), &token).await;

    // Rutas que fallan: tipo malo, 404, tamaño, almacén roto y base rota.
    let mut malo = format!("{TEXTO_SECRETO} no es un pdf").into_bytes();
    malo.extend_from_slice(b"\x00\x01");
    respuestas.push(
        subir_crudo(&app, Some(&token), &nombre, &malo)
            .await
            .texto(),
    );
    let mut enorme = pdf_de_tamano(MB5 + 1);
    enorme.extend_from_slice(TEXTO_SECRETO.as_bytes());
    respuestas.push(
        subir_crudo(&app, Some(&token), &nombre, &enorme)
            .await
            .texto(),
    );
    respuestas.push(
        simple(&app, "GET", &format!("{CVS}/{NOMBRE_SECRETO}/file"), &token)
            .await
            .texto(),
    );
    almacen.falla_put.store(true, Ordering::SeqCst);
    respuestas.push(
        subir_crudo(&app, Some(&token), &nombre, &datos)
            .await
            .texto(),
    );
    almacen.falla_put.store(false, Ordering::SeqCst);
    db.exec("DROP TABLE cv_extractions").await.unwrap();
    respuestas.push(
        subir_crudo(&app, Some(&token), &nombre, &datos)
            .await
            .texto(),
    );
    almacen.falla_delete.store(true, Ordering::SeqCst);
    respuestas.push(
        subir_crudo(&app, Some(&token), &nombre, &datos)
            .await
            .texto(),
    );
    simple(&app, "GET", CVS, &token).await;
    respuestas.push(
        simple(&app, "DELETE", &format!("{CVS}/{id}"), &token)
            .await
            .texto(),
    );

    let texto_log = log.texto();
    assert!(!texto_log.is_empty(), "no se capturó ningún log");
    // El log de sqlx nombra columnas (`storage_key`) pero nunca valores: lo que no puede
    // aparecer en ningún lado es el valor.
    for fuente in respuestas.iter().chain([&texto_log]) {
        for secreto in [NOMBRE_SECRETO, TEXTO_SECRETO, clave.as_str()] {
            assert!(!fuente.contains(secreto), "se filtró {secreto:?}: {fuente}");
        }
    }
    for respuesta in &respuestas {
        assert!(!respuesta.contains("storage_key"), "{respuesta}");
    }
    db.cleanup().await;
}

/// S6: un cuerpo infinito se corta en el tope, sin agotar memoria.
#[tokio::test]
async fn s6_un_cuerpo_infinito_se_corta_en_el_tope() {
    let (db, app, almacen, token) = entorno().await;
    let leido = Arc::new(AtomicUsize::new(0));
    let contador = leido.clone();
    let (tipo, mut inicio) = multipart(&[("file", Some("infinito.pdf"), b"%PDF-1.4\n")]);
    // Quitar el cierre del multipart: el archivo no termina nunca.
    let cierre = format!("\r\n--{LIMITE}--\r\n").len();
    inicio.truncate(inicio.len() - cierre);
    let flujo = futures_util::stream::iter([Ok::<_, Infallible>(Bytes::from(inicio))]).chain(
        futures_util::stream::unfold((), move |_| {
            let contador = contador.clone();
            async move {
                contador.fetch_add(64 * 1024, Ordering::SeqCst);
                Some((Ok::<_, Infallible>(Bytes::from(vec![b'x'; 64 * 1024])), ()))
            }
        }),
    );
    let r = crudo(
        &app,
        "POST",
        CVS,
        Some(&token),
        Some(&tipo),
        Body::from_stream(flujo),
    )
    .await;
    assert_eq!(r.estado, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(r.json()["error"], "archivo_demasiado_grande");
    let total = leido.load(Ordering::SeqCst);
    assert!(
        total <= 6 * 1024 * 1024 + 256 * 1024,
        "se leyeron {total} bytes de un cuerpo infinito"
    );
    assert!(almacen.claves().is_empty());
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 0);
    db.cleanup().await;
}

/// S6b: un `Content-Length` mentiroso (dice poco, trae mucho) no evita el tope.
#[tokio::test]
async fn s6b_un_content_length_mentiroso_no_evita_el_tope() {
    let (db, app, almacen, token) = entorno().await;
    let (tipo, cuerpo) = multipart(&[("file", Some("a.pdf"), &pdf_de_tamano(7 * 1024 * 1024))]);
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(CVS)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", tipo)
                .header("content-length", "100")
                .body(Body::from(cuerpo))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(almacen.claves().is_empty());
    db.cleanup().await;
}

/// S8: el texto extraído no se guarda en ninguna tabla.
#[tokio::test]
async fn s8_el_texto_extraido_no_se_guarda() {
    let (db, app, _a, token) = entorno().await;
    let marca = "ZETAMARCAUNICAXYZ";
    let (estado, c) = subir(
        &app,
        &token,
        "a.pdf",
        &cv_pdf(&format!("Rust y Python. Aficion: {marca} {marca}")),
    )
    .await;
    assert_eq!(estado, StatusCode::CREATED);
    assert_eq!(c["extraction"]["status"], "ready");
    let tablas = db
        .conn
        .query_all(sea_orm::Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT table_name::text FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_type = 'BASE TABLE'"
                .to_string(),
        ))
        .await
        .unwrap();
    assert!(tablas.len() > 10);
    for t in tablas {
        let t: String = t.try_get_by_index(0).unwrap();
        let n = db
            .count(&format!(
                "SELECT count(*) FROM \"{t}\" x WHERE x::text ILIKE '%{marca}%'"
            ))
            .await;
        assert_eq!(n, 0, "el texto del CV apareció en la tabla {t}");
    }
    db.cleanup().await;
}

/// La cuenta borrada se lleva sus CVs de la base (FK en cascada) y ya no puede pedirlos.
#[tokio::test]
async fn borrar_la_cuenta_borra_las_filas_de_cv() {
    let (db, app, _a, token) = entorno().await;
    subir(&app, &token, "a.pdf", &cv_pdf("x")).await;
    db.exec("DELETE FROM users").await.unwrap();
    assert_eq!(db.count("SELECT count(*) FROM cvs").await, 0);
    let r = simple(&app, "GET", CVS, &token).await;
    assert_eq!(r.estado, StatusCode::UNAUTHORIZED);
    db.cleanup().await;
}

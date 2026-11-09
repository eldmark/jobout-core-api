//! Rutas de CV: subir, listar, descargar, elegir el primario y borrar.
//!
//! - El usuario sale siempre del token (`AuthUser`) y toda consulta lo exige en el `WHERE`: un CV
//!   ajeno, inexistente o con id mal formado es el mismo `404 no_encontrado`, nunca 403.
//! - El tipo del archivo se decide **por contenido** (`cv::text::detect_kind`); el nombre que
//!   manda el cliente solo se sanea para mostrarlo y nunca forma parte de la clave del almacén.
//! - La clave del almacén es aleatoria y va **cifrada** en `cvs.storage_key`.
//! - El texto extraído vive solo en memoria durante la petición: no se guarda ni se registra.
//! - Todo o nada: si falla el almacén no queda fila; si falla la base tras guardar el archivo,
//!   el archivo se borra (y si ese borrado también falla, la clave va a
//!   `pending_storage_deletions`, cifrada).
//! - Nada del nombre, el contenido, el texto ni la clave se escribe en un log; los errores
//!   internos se registran por categoría.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{multipart::MultipartRejection, Multipart, Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
    Json,
};
use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement, TransactionTrait, Value};
use serde_json::{json, Value as Json_};

use super::table::{Access, RouteSpec};
use crate::{
    auth_user::AuthUser,
    blob::new_blob_key,
    cv::{
        extractor::{CvExtractor, DeterministicExtractor, LanguageEntry, Proposal, SkillEntry},
        text::{detect_kind, extract_text, run_with_timeout, CvKind, Limits, TextError},
    },
    state::AppState,
};

/// Tamaño máximo de un CV.
const MAX_CV_BYTES: usize = 5 * 1024 * 1024;
/// Tope del cuerpo de la subida: el CV más el sobre del multipart.
const UPLOAD_BODY_LIMIT: usize = 6 * 1024 * 1024;
/// CVs guardados como máximo por persona.
const MAX_CVS: i64 = 5;
/// Tiempo máximo de la extracción dentro de la petición.
const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_FILENAME_CHARS: usize = 120;
const MODEL: &str = "deterministic-v1";
const MIME_PDF: &str = "application/pdf";
const MIME_DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

pub fn routes() -> Vec<RouteSpec> {
    vec![
        RouteSpec {
            path: "/api/v1/cvs",
            methods: &["GET", "POST"],
            access: Access::Authenticated,
            body_limit: Some(UPLOAD_BODY_LIMIT),
            handler: get(list).post(upload),
        },
        RouteSpec {
            path: "/api/v1/cvs/{id}",
            methods: &["DELETE"],
            access: Access::Authenticated,
            body_limit: None,
            handler: axum::routing::delete(remove),
        },
        RouteSpec {
            path: "/api/v1/cvs/{id}/file",
            methods: &["GET"],
            access: Access::Authenticated,
            body_limit: None,
            handler: get(download),
        },
        RouteSpec {
            path: "/api/v1/cvs/{id}/primary",
            methods: &["PUT"],
            access: Access::Authenticated,
            body_limit: None,
            handler: put(make_primary),
        },
    ]
}

// ------------------------------------------------------------------ errores

enum ApiError {
    Invalid,
    TooLarge,
    Unsupported,
    LimitReached,
    NotFound,
    Internal,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (estado, codigo, mensaje) = match self {
            Self::Invalid => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "datos_invalidos",
                "Envía un único archivo en el campo «file».",
            ),
            Self::TooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "archivo_demasiado_grande",
                "El archivo supera el máximo de 5 MB.",
            ),
            Self::Unsupported => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "tipo_no_permitido",
                "Solo se aceptan archivos PDF o DOCX.",
            ),
            Self::LimitReached => (
                StatusCode::CONFLICT,
                "limite_de_cvs",
                "Ya tienes 5 CVs guardados. Borra uno para poder subir otro.",
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "no_encontrado",
                "No encontramos ese CV.",
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "error_interno",
                "Algo salió mal de nuestro lado. Inténtalo de nuevo.",
            ),
        };
        (estado, Json(json!({"error": codigo, "message": mensaje}))).into_response()
    }
}

/// Registra el fallo por categoría, **nunca** con el texto del error de la base.
fn db(donde: &'static str) -> impl Fn(DbErr) -> ApiError {
    move |_| {
        tracing::error!(donde, "error interno");
        ApiError::Internal
    }
}

fn interno(donde: &'static str) -> ApiError {
    tracing::error!(donde, "error interno");
    ApiError::Internal
}

fn stmt(sql: &str, valores: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, valores)
}

// ------------------------------------------------------------------ piezas puras

/// Un id de CV válido (UUID en texto) o `None`. Se comprueba antes de tocar la base para que un
/// id basura sea 404 y no un error del `::uuid`.
fn valid_id(id: &str) -> Option<&str> {
    let b = id.as_bytes();
    let ok = b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        });
    ok.then_some(id)
}

/// Nombre apto para **mostrar**: sin directorios, sin `..`, sin caracteres de control ni de
/// dirección de texto, sin comillas, de 120 caracteres como mucho.
fn sanitize_filename(original: Option<&str>, kind: CvKind) -> String {
    let base = original
        .unwrap_or("")
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("");
    let mut limpio: String = base
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '"' | '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        })
        .collect();
    while limpio.contains("..") {
        limpio = limpio.replace("..", ".");
    }
    let limpio: String = limpio
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .chars()
        .take(MAX_FILENAME_CHARS)
        .collect();
    let limpio = limpio.trim_end().to_string();
    if limpio.is_empty() {
        match kind {
            CvKind::Pdf => "cv.pdf".to_string(),
            CvKind::Docx => "cv.docx".to_string(),
        }
    } else {
        limpio
    }
}

/// `Content-Disposition: attachment; filename*=UTF-8''…` con todo lo no seguro codificado.
fn content_disposition(nombre: &str) -> String {
    let mut s = String::from("attachment; filename*=UTF-8''");
    for b in nombre.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

fn mime_of(kind: CvKind) -> &'static str {
    match kind {
        CvKind::Pdf => MIME_PDF,
        CvKind::Docx => MIME_DOCX,
    }
}

fn reason_of(error: TextError) -> &'static str {
    match error {
        TextError::NoText => "sin_texto",
        TextError::Corrupt | TextError::TooLarge => "ilegible",
        TextError::TimedOut => "tiempo_agotado",
    }
}

// ------------------------------------------------------------------ lectura del cuerpo

fn multipart_error(e: axum::extract::multipart::MultipartError) -> ApiError {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::TooLarge
    } else {
        ApiError::Invalid
    }
}

/// Lee el único campo `file`, cortando al pasar del tope: un cuerpo infinito no agota memoria.
async fn read_file(
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<(Option<String>, Vec<u8>), ApiError> {
    let mut multipart = multipart.map_err(|_| ApiError::Invalid)?;
    let mut archivo: Option<(Option<String>, Vec<u8>)> = None;
    while let Some(mut campo) = multipart.next_field().await.map_err(multipart_error)? {
        if campo.name() != Some("file") || archivo.is_some() {
            return Err(ApiError::Invalid);
        }
        let nombre = campo.file_name().map(str::to_owned);
        let mut datos: Vec<u8> = Vec::new();
        while let Some(trozo) = campo.chunk().await.map_err(multipart_error)? {
            if datos.len() + trozo.len() > MAX_CV_BYTES {
                return Err(ApiError::TooLarge);
            }
            datos.extend_from_slice(&trozo);
        }
        archivo = Some((nombre, datos));
    }
    archivo.ok_or(ApiError::Invalid)
}

// ------------------------------------------------------------------ extracción

enum Extraction {
    Ready(Proposal),
    Failed(&'static str),
}

async fn load_catalogs(
    state: &AppState,
) -> Result<(Vec<SkillEntry>, Vec<LanguageEntry>), ApiError> {
    let skills = state
        .db
        .query_all(stmt(
            "SELECT id::text, code, name, aliases FROM skills ORDER BY code",
            vec![],
        ))
        .await
        .map_err(db("leer_skills"))?;
    let skills = skills
        .iter()
        .map(|f| {
            Ok(SkillEntry {
                id: f.try_get_by_index(0)?,
                code: f.try_get_by_index(1)?,
                name: f.try_get_by_index(2)?,
                aliases: f.try_get_by_index(3)?,
            })
        })
        .collect::<Result<Vec<_>, DbErr>>()
        .map_err(db("leer_fila_skill"))?;
    let idiomas = state
        .db
        .query_all(stmt(
            "SELECT id::text, code, name FROM languages ORDER BY code",
            vec![],
        ))
        .await
        .map_err(db("leer_idiomas"))?;
    let idiomas = idiomas
        .iter()
        .map(|f| {
            Ok(LanguageEntry {
                id: f.try_get_by_index(0)?,
                code: f.try_get_by_index(1)?,
                name: f.try_get_by_index(2)?,
            })
        })
        .collect::<Result<Vec<_>, DbErr>>()
        .map_err(db("leer_fila_idioma"))?;
    Ok((skills, idiomas))
}

/// Extrae y propone, con tope de tiempo. El texto no sale de esta función.
async fn extract(
    datos: Arc<Vec<u8>>,
    kind: CvKind,
    skills: Vec<SkillEntry>,
    idiomas: Vec<LanguageEntry>,
) -> Extraction {
    let trabajo = tokio::task::spawn_blocking(move || {
        run_with_timeout(EXTRACTION_TIMEOUT, move || {
            let texto = extract_text(&datos, kind, &Limits::default())?;
            Ok::<Proposal, TextError>(
                DeterministicExtractor::new(skills, idiomas).extract(&texto.text),
            )
        })
    })
    .await;
    match trabajo {
        Ok(Ok(Ok(propuesta))) => Extraction::Ready(propuesta),
        Ok(Ok(Err(e)) | Err(e)) => Extraction::Failed(reason_of(e)),
        Err(_) => Extraction::Failed("ilegible"),
    }
}

// ------------------------------------------------------------------ subida

/// Bloquea la fila del candidato: serializa subidas, borrados y cambios de primario de la misma
/// persona. Devuelve el id del applicant.
async fn lock_applicant(txn: &impl ConnectionTrait, user_id: &str) -> Result<String, ApiError> {
    txn.query_one(stmt(
        "SELECT id::text FROM applicants WHERE user_id = $1::uuid FOR UPDATE",
        vec![user_id.into()],
    ))
    .await
    .map_err(db("bloquear_applicant"))?
    .ok_or_else(|| interno("sin_applicant"))?
    .try_get_by_index(0)
    .map_err(db("leer_applicant"))
}

/// Borra un archivo que no debe quedar; si no se puede, deja su clave (cifrada) en la bandeja.
async fn discard_blob(state: &AppState, clave: &str) {
    if state.blobs.delete(clave).await.is_ok() {
        return;
    }
    tracing::error!(donde = "borrar_archivo_huerfano", "error interno");
    let Ok(cifrada) = state.pii.encrypt(clave) else {
        tracing::error!(donde = "cifrar_clave_pendiente", "error interno");
        return;
    };
    let guardado = state
        .db
        .execute(stmt(
            "INSERT INTO pending_storage_deletions (id, storage_key, attempts, last_attempt_at) \
             VALUES ($1::uuid, $2, 1, now())",
            vec![migration::new_id().to_string().into(), cifrada.into()],
        ))
        .await;
    if guardado.is_err() {
        tracing::error!(donde = "guardar_pendiente", "error interno");
    }
}

async fn upload(
    State(state): State<AppState>,
    user: AuthUser,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<Json_>), ApiError> {
    let (nombre_original, datos) = read_file(multipart).await?;
    let kind = detect_kind(&datos).ok_or(ApiError::Unsupported)?;
    let nombre = sanitize_filename(nombre_original.as_deref(), kind);
    let size = datos.len() as i64;

    let (skills, idiomas) = load_catalogs(&state).await?;
    let datos = Arc::new(datos);
    let extraccion = extract(datos.clone(), kind, skills, idiomas).await;
    let (estado, razon, propuesta) = match &extraccion {
        Extraction::Ready(p) => ("ready", None, Some(p)),
        Extraction::Failed(r) => ("failed", Some(*r), None),
    };
    let extracted = match propuesta {
        Some(p) => serde_json::to_string(p).map_err(|_| interno("serializar_propuesta"))?,
        None => "{}".to_string(),
    };

    let txn = state.db.begin().await.map_err(db("begin"))?;
    let applicant = lock_applicant(&txn, &user.id).await?;
    let fila = txn
        .query_one(stmt(
            "SELECT count(*), COALESCE(max(version), 0)::bigint FROM cvs \
             WHERE applicant_id = $1::uuid",
            vec![applicant.clone().into()],
        ))
        .await
        .map_err(db("contar_cvs"))?
        .ok_or_else(|| interno("contar_cvs_vacio"))?;
    let cantidad: i64 = fila.try_get_by_index(0).map_err(db("leer_cuenta"))?;
    let ultima: i64 = fila.try_get_by_index(1).map_err(db("leer_version"))?;
    if cantidad >= MAX_CVS {
        return Err(ApiError::LimitReached);
    }

    let clave = new_blob_key();
    let clave_cifrada = state
        .pii
        .encrypt(&clave)
        .map_err(|_| interno("cifrar_clave"))?;
    state.blobs.put(&clave, &datos).await.map_err(|_| {
        tracing::error!(donde = "guardar_archivo", "error interno");
        ApiError::Internal
    })?;

    // Desde acá hay un archivo guardado: cualquier fallo lo tiene que borrar.
    let id = migration::new_id().to_string();
    let guardar = async {
        txn.execute(stmt(
            "UPDATE cvs SET is_primary = false, updated_at = now() \
             WHERE applicant_id = $1::uuid AND is_primary",
            vec![applicant.clone().into()],
        ))
        .await
        .map_err(db("quitar_primario"))?;
        let creado: String = txn
            .query_one(stmt(
                "INSERT INTO cvs (id, applicant_id, storage_key, original_filename, mime_type, \
                                  size_bytes, version, is_primary) \
                 VALUES ($1::uuid, $2::uuid, $3, $4, $5, $6, $7, true) \
                 RETURNING to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')",
                vec![
                    id.clone().into(),
                    applicant.clone().into(),
                    clave_cifrada.into(),
                    nombre.clone().into(),
                    mime_of(kind).into(),
                    size.into(),
                    ((ultima + 1) as i32).into(),
                ],
            ))
            .await
            .map_err(db("insertar_cv"))?
            .ok_or_else(|| interno("insertar_cv_vacio"))?
            .try_get_by_index(0)
            .map_err(db("leer_fecha"))?;
        txn.execute(stmt(
            "INSERT INTO cv_extractions (id, cv_id, extracted, status, reason, model) \
             VALUES ($1::uuid, $2::uuid, $3::jsonb, $4::cv_extraction_status, $5, $6)",
            vec![
                migration::new_id().to_string().into(),
                id.clone().into(),
                extracted.into(),
                estado.into(),
                razon.map(str::to_string).into(),
                MODEL.into(),
            ],
        ))
        .await
        .map_err(db("insertar_extraccion"))?;
        Ok::<String, ApiError>(creado)
    }
    .await;
    let creado = match guardar {
        Ok(c) => match txn.commit().await {
            Ok(()) => Ok(c),
            Err(_) => Err(interno("commit")),
        },
        Err(e) => {
            let _ = txn.rollback().await;
            Err(e)
        }
    };
    let creado = match creado {
        Ok(c) => c,
        Err(e) => {
            discard_blob(&state, &clave).await;
            return Err(e);
        }
    };

    let proposal = propuesta
        .map(serde_json::to_value)
        .transpose()
        .map_err(|_| interno("serializar_respuesta"))?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "version": ultima + 1,
            "is_primary": true,
            "original_filename": nombre,
            "size_bytes": size,
            "created_at": creado,
            "extraction": {"status": estado, "reason": razon, "proposal": proposal},
        })),
    ))
}

// ------------------------------------------------------------------ lista y descarga

async fn list(State(state): State<AppState>, user: AuthUser) -> Result<Json<Json_>, ApiError> {
    let filas = state
        .db
        .query_all(stmt(
            "SELECT c.id::text, c.version, c.is_primary, c.original_filename, c.size_bytes, \
                    to_char(c.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') \
             FROM cvs c JOIN applicants a ON a.id = c.applicant_id \
             WHERE a.user_id = $1::uuid ORDER BY c.version DESC",
            vec![user.id.into()],
        ))
        .await
        .map_err(db("listar_cvs"))?;
    let mut salida = Vec::with_capacity(filas.len());
    for f in &filas {
        let fila = (|| -> Result<Json_, DbErr> {
            Ok(json!({
                "id": f.try_get_by_index::<String>(0)?,
                "version": f.try_get_by_index::<i32>(1)?,
                "is_primary": f.try_get_by_index::<bool>(2)?,
                "original_filename": f.try_get_by_index::<String>(3)?,
                "size_bytes": f.try_get_by_index::<i64>(4)?,
                "created_at": f.try_get_by_index::<String>(5)?,
            }))
        })()
        .map_err(db("leer_fila_cv"))?;
        salida.push(fila);
    }
    Ok(Json(Json_::Array(salida)))
}

async fn download(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = valid_id(&id).ok_or(ApiError::NotFound)?;
    let fila = state
        .db
        .query_one(stmt(
            "SELECT c.storage_key, c.mime_type, c.original_filename \
             FROM cvs c JOIN applicants a ON a.id = c.applicant_id \
             WHERE c.id = $1::uuid AND a.user_id = $2::uuid",
            vec![id.into(), user.id.into()],
        ))
        .await
        .map_err(db("leer_cv"))?
        .ok_or(ApiError::NotFound)?;
    let cifrada: Vec<u8> = fila.try_get_by_index(0).map_err(db("leer_clave"))?;
    let mime: String = fila.try_get_by_index(1).map_err(db("leer_mime"))?;
    let nombre: String = fila.try_get_by_index(2).map_err(db("leer_nombre"))?;
    let clave = state
        .pii
        .decrypt(&cifrada)
        .map_err(|_| interno("descifrar_clave"))?;
    let bytes = state.blobs.get(&clave).await.map_err(|_| {
        tracing::error!(donde = "leer_archivo", "error interno");
        ApiError::Internal
    })?;

    let mut respuesta = (StatusCode::OK, bytes).into_response();
    let cabeceras = respuesta.headers_mut();
    cabeceras.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime).map_err(|_| interno("mime_invalido"))?,
    );
    cabeceras.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(&nombre))
            .map_err(|_| interno("disposicion_invalida"))?,
    );
    cabeceras.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    cabeceras.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(respuesta)
}

// ------------------------------------------------------------------ primario y borrado

async fn make_primary(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = valid_id(&id).ok_or(ApiError::NotFound)?;
    let txn = state.db.begin().await.map_err(db("begin"))?;
    let applicant = lock_applicant(&txn, &user.id).await?;
    txn.query_one(stmt(
        "SELECT 1 FROM cvs WHERE id = $1::uuid AND applicant_id = $2::uuid",
        vec![id.into(), applicant.clone().into()],
    ))
    .await
    .map_err(db("buscar_cv"))?
    .ok_or(ApiError::NotFound)?;
    // Primero se suelta el actual: el índice único admite un solo primario a la vez.
    txn.execute(stmt(
        "UPDATE cvs SET is_primary = false, updated_at = now() \
         WHERE applicant_id = $1::uuid AND is_primary AND id <> $2::uuid",
        vec![applicant.into(), id.into()],
    ))
    .await
    .map_err(db("quitar_primario"))?;
    txn.execute(stmt(
        "UPDATE cvs SET is_primary = true, updated_at = now() WHERE id = $1::uuid",
        vec![id.into()],
    ))
    .await
    .map_err(db("poner_primario"))?;
    txn.commit().await.map_err(db("commit"))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = valid_id(&id).ok_or(ApiError::NotFound)?;
    let txn = state.db.begin().await.map_err(db("begin"))?;
    let applicant = lock_applicant(&txn, &user.id).await?;
    let fila = txn
        .query_one(stmt(
            "SELECT storage_key, is_primary FROM cvs \
             WHERE id = $1::uuid AND applicant_id = $2::uuid",
            vec![id.into(), applicant.clone().into()],
        ))
        .await
        .map_err(db("buscar_cv"))?
        .ok_or(ApiError::NotFound)?;
    let cifrada: Vec<u8> = fila.try_get_by_index(0).map_err(db("leer_clave"))?;
    let era_primario: bool = fila.try_get_by_index(1).map_err(db("leer_primario"))?;

    // La clave se anota en la bandeja en la misma transacción: si el proceso muere entre el
    // commit y el borrado del archivo, no se pierde. Se quita al borrar el archivo.
    let pendiente = migration::new_id().to_string();
    txn.execute(stmt(
        "INSERT INTO pending_storage_deletions (id, storage_key) VALUES ($1::uuid, $2)",
        vec![pendiente.clone().into(), cifrada.clone().into()],
    ))
    .await
    .map_err(db("anotar_pendiente"))?;
    // La extracción cae en cascada.
    txn.execute(stmt("DELETE FROM cvs WHERE id = $1::uuid", vec![id.into()]))
        .await
        .map_err(db("borrar_cv"))?;
    if era_primario {
        txn.execute(stmt(
            "UPDATE cvs SET is_primary = true, updated_at = now() WHERE id = ( \
                 SELECT id FROM cvs WHERE applicant_id = $1::uuid ORDER BY version DESC LIMIT 1)",
            vec![applicant.into()],
        ))
        .await
        .map_err(db("promover_primario"))?;
    }
    txn.commit().await.map_err(db("commit"))?;

    // El CV ya no existe para la persona; el archivo se borra aparte y, si falla, queda anotado.
    let clave = state
        .pii
        .decrypt(&cifrada)
        .map_err(|_| interno("descifrar_clave"))?;
    let (sql, donde) = if state.blobs.delete(&clave).await.is_ok() {
        (
            "DELETE FROM pending_storage_deletions WHERE id = $1::uuid",
            "limpiar_pendiente",
        )
    } else {
        tracing::error!(donde = "borrar_archivo", "error interno");
        (
            "UPDATE pending_storage_deletions \
             SET attempts = attempts + 1, last_attempt_at = now() WHERE id = $1::uuid",
            "marcar_pendiente",
        )
    };
    if state
        .db
        .execute(stmt(sql, vec![pendiente.into()]))
        .await
        .is_err()
    {
        tracing::error!(donde, "error interno");
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_razon_de_cada_error_de_texto() {
        assert_eq!(reason_of(TextError::NoText), "sin_texto");
        assert_eq!(reason_of(TextError::Corrupt), "ilegible");
        assert_eq!(reason_of(TextError::TooLarge), "ilegible");
        assert_eq!(reason_of(TextError::TimedOut), "tiempo_agotado");
    }

    #[test]
    fn solo_se_aceptan_uuid_como_id() {
        assert!(valid_id("00000000-0000-7000-8000-000000000000").is_some());
        assert!(valid_id("0000000000007000800000000000000000000").is_none());
        assert!(valid_id("00000000-0000-7000-8000-00000000000g").is_none());
        assert!(valid_id("").is_none());
    }

    #[test]
    fn el_nombre_se_sanea() {
        let s = |n: &str| sanitize_filename(Some(n), CvKind::Pdf);
        assert_eq!(s("../../etc/passwd"), "passwd");
        assert_eq!(s("a/..b..c.pdf"), "b.c.pdf");
        assert_eq!(s("   "), "cv.pdf");
        assert_eq!(sanitize_filename(None, CvKind::Docx), "cv.docx");
        assert_eq!(s(&"a".repeat(500)).chars().count(), 120);
        assert_eq!(s("x\"y.pdf"), "xy.pdf");
    }

    #[test]
    fn el_encabezado_de_descarga_codifica_lo_no_seguro() {
        assert_eq!(
            content_disposition("ñ a.pdf"),
            "attachment; filename*=UTF-8''%C3%B1%20a.pdf"
        );
    }
}

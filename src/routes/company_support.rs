//! Lo que comparten las rutas de empresas, invitaciones y administración de la plataforma:
//! errores del contrato, lectura de cuerpos y consultas **cerrados**, paginación con cursor
//! opaco, el contador de intentos, la auditoría y la IP del cliente.
//!
//! Ningún texto de error repite lo recibido (ni el valor ni el nombre del campo o parámetro) y
//! los fallos internos se registran por categoría, nunca con el texto de la base.

use axum::{
    body::Bytes,
    extract::{ConnectInfo, FromRequestParts},
    http::{header::RETRY_AFTER, request::Parts, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sea_orm::{ConnectionTrait, DbBackend, DbErr, SqlErr, Statement, Value};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::{collections::HashMap, convert::Infallible, net::SocketAddr};
use uuid::Uuid;

use crate::crypto::PiiKeys;

pub enum ApiError {
    NotFound,
    /// 404 `invitacion_invalida`: misma respuesta para las cinco causas.
    InvalidInvitation,
    /// 403 `sin_permiso` (el mismo cuerpo que da la tabla de rutas).
    NoPermission,
    /// 422 `datos_invalidos`, con texto fijo.
    Invalid(&'static str),
    /// 422 `campo_no_permitido`.
    FieldNotAllowed,
    /// 422 `parametro_no_permitido`.
    ParamNotAllowed,
    /// 409 con su código y texto fijos.
    Conflict(&'static str, &'static str),
    /// 429 `demasiados_intentos` con `Retry-After` en segundos.
    TooMany(u64),
    Internal,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (estado, codigo, mensaje) = match self {
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "no_encontrado",
                "No encontramos lo que buscas.",
            ),
            Self::InvalidInvitation => (
                StatusCode::NOT_FOUND,
                "invitacion_invalida",
                "La invitación no es válida.",
            ),
            Self::NoPermission => (
                StatusCode::FORBIDDEN,
                "sin_permiso",
                "No tienes permiso para hacer esto.",
            ),
            Self::Invalid(m) => (StatusCode::UNPROCESSABLE_ENTITY, "datos_invalidos", m),
            Self::FieldNotAllowed => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "campo_no_permitido",
                "El cuerpo trae un campo que no se admite.",
            ),
            Self::ParamNotAllowed => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "parametro_no_permitido",
                "La consulta trae un parámetro que no se admite.",
            ),
            Self::Conflict(codigo, m) => (StatusCode::CONFLICT, codigo, m),
            Self::TooMany(segundos) => {
                let mut r = (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({
                        "error": "demasiados_intentos",
                        "message": "Demasiados intentos. Espera un momento e inténtalo de nuevo."
                    })),
                )
                    .into_response();
                if let Ok(v) = HeaderValue::from_str(&segundos.max(1).to_string()) {
                    r.headers_mut().insert(RETRY_AFTER, v);
                }
                return r;
            }
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "error_interno",
                "Algo salió mal de nuestro lado. Inténtalo de nuevo.",
            ),
        };
        (estado, Json(json!({"error": codigo, "message": mensaje}))).into_response()
    }
}

pub const YA_TIENE_EMPRESA: ApiError = ApiError::Conflict(
    "ya_tiene_empresa",
    "Esta cuenta ya pertenece a una empresa.",
);
pub const ULTIMO_ADMIN: ApiError = ApiError::Conflict(
    "ultimo_administrador",
    "La empresa necesita al menos un administrador.",
);

/// Registra el fallo por categoría, **nunca** con el texto del error de la base.
pub fn db(donde: &'static str) -> impl Fn(DbErr) -> ApiError {
    move |_| {
        tracing::error!(donde, "error interno");
        ApiError::Internal
    }
}

pub fn interno(donde: &'static str) -> ApiError {
    tracing::error!(donde, "error interno");
    ApiError::Internal
}

pub fn stmt(sql: &str, valores: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, valores)
}

/// ¿El error es una violación de unicidad de esa restricción o índice? Mira el nombre, nunca lo
/// escribe en un log.
pub fn viola_unico(e: &DbErr, restriccion: &str) -> bool {
    match e.sql_err() {
        Some(SqlErr::UniqueConstraintViolation(msg)) => msg.contains(restriccion),
        _ => false,
    }
}

// ------------------------------------------------------------------ SQL de apoyo

/// ISO 8601 con zona (UTC) de una columna `timestamptz`.
pub fn iso(col: &str) -> String {
    format!("to_char({col} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')")
}

/// Microsegundos Unix de una columna `timestamptz` (exactos, para el cursor).
pub fn micros(col: &str) -> String {
    format!("(extract(epoch from {col}) * 1000000)::bigint")
}

/// Un instante a partir de microsegundos Unix, en el parámetro `$n`.
pub fn desde_micros(n: usize) -> String {
    format!("(timestamptz 'epoch' + ${n}::bigint * interval '1 microsecond')")
}

// ------------------------------------------------------------------ cuerpos y consultas

/// Lee un cuerpo JSON **cerrado**: tiene que ser un objeto, sin claves fuera de `permitidos`
/// (422 `campo_no_permitido`) y con los tipos del contrato (422 `datos_invalidos`). El
/// deserializador no habla: sus mensajes pueden citar el dato.
pub fn parse_cerrado<T: DeserializeOwned>(
    cuerpo: &Bytes,
    permitidos: &[&str],
) -> Result<T, ApiError> {
    let invalido = || ApiError::Invalid("El cuerpo no tiene el formato esperado.");
    let valor: serde_json::Value = serde_json::from_slice(cuerpo).map_err(|_| invalido())?;
    let objeto = valor.as_object().ok_or_else(invalido)?;
    if objeto.keys().any(|k| !permitidos.contains(&k.as_str())) {
        return Err(ApiError::FieldNotAllowed);
    }
    serde_json::from_value(valor).map_err(|_| invalido())
}

/// UUID en un path: uno mal formado es lo mismo que uno inexistente.
pub fn id_de_ruta(s: &str) -> Result<String, ApiError> {
    super::profile::uuid_canonico(s).ok_or(ApiError::NotFound)
}

/// Texto libre opcional: recortado, vacío = ausente, con tope de caracteres.
pub fn texto_opcional(
    t: &Option<String>,
    max: usize,
    mensaje: &'static str,
) -> Result<Option<String>, ApiError> {
    match t.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) if s.chars().count() > max => Err(ApiError::Invalid(mensaje)),
        Some(s) => Ok(Some(s.to_string())),
    }
}

fn pct_decodificar(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut salida = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => salida.push(b' '),
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                salida.push(u8::from_str_radix(hex, 16).ok()?);
                i += 2;
            }
            c => salida.push(c),
        }
        i += 1;
    }
    String::from_utf8(salida).ok()
}

/// Lee la consulta **cerrada**: un parámetro fuera de `permitidos` es 422 `parametro_no_permitido`
/// (antes de tocar la base y sin repetir el nombre); uno repetido o mal codificado, `datos_invalidos`.
pub fn parse_consulta(
    cruda: Option<&str>,
    permitidos: &[&str],
) -> Result<HashMap<String, String>, ApiError> {
    let mut mapa = HashMap::new();
    let mut desconocido = false;
    let mut invalido = false;
    for par in cruda.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (k, v) = par.split_once('=').unwrap_or((par, ""));
        match (pct_decodificar(k), pct_decodificar(v)) {
            (Some(k), Some(v)) => {
                if !permitidos.contains(&k.as_str()) {
                    desconocido = true;
                } else if mapa.insert(k, v).is_some() {
                    invalido = true;
                }
            }
            _ => invalido = true,
        }
    }
    if desconocido {
        return Err(ApiError::ParamNotAllowed);
    }
    if invalido {
        return Err(ApiError::Invalid("La consulta no es válida."));
    }
    Ok(mapa)
}

/// `limit` (1–50, por defecto 20) y `cursor` opaco: instante en microsegundos e id.
pub struct Pagina {
    pub limite: i64,
    pub cursor: Option<(i64, String)>,
}

impl Pagina {
    pub fn de(consulta: &HashMap<String, String>) -> Result<Self, ApiError> {
        let malo = || ApiError::Invalid("La paginación no es válida.");
        let limite = match consulta.get("limit") {
            None => 20,
            Some(v) => v
                .parse::<i64>()
                .ok()
                .filter(|n| (1..=50).contains(n) && v.bytes().all(|b| b.is_ascii_digit()))
                .ok_or_else(malo)?,
        };
        let cursor = match consulta.get("cursor") {
            None => None,
            Some(c) => Some(decodificar_cursor(c).ok_or_else(malo)?),
        };
        Ok(Self { limite, cursor })
    }
}

pub fn codificar_cursor(micros: i64, id: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("{micros}:{id}"))
}

fn decodificar_cursor(c: &str) -> Option<(i64, String)> {
    let bytes = URL_SAFE_NO_PAD.decode(c).ok()?;
    let texto = String::from_utf8(bytes).ok()?;
    let (m, id) = texto.split_once(':')?;
    let id = Uuid::parse_str(id).ok()?.to_string();
    Some((m.parse().ok()?, id))
}

// ------------------------------------------------------------------ empresa de quien llama

pub struct MiEmpresa {
    pub company_id: String,
    pub role: String,
}

/// La empresa sale de la membresía de quien llama, nunca del path ni del cuerpo.
pub async fn mi_empresa(
    conn: &impl ConnectionTrait,
    user_id: &str,
) -> Result<Option<MiEmpresa>, ApiError> {
    let fila = conn
        .query_one(stmt(
            "SELECT cr.company_id::text, cr.role \
             FROM company_recruiters cr \
             JOIN companies c ON c.id = cr.company_id AND c.deleted_at IS NULL \
             WHERE cr.user_id = $1::uuid AND cr.deleted_at IS NULL",
            vec![user_id.to_string().into()],
        ))
        .await
        .map_err(db("mi_empresa"))?;
    let Some(f) = fila else { return Ok(None) };
    Ok(Some(MiEmpresa {
        company_id: f.try_get_by_index(0).map_err(db("mi_empresa_id"))?,
        role: f.try_get_by_index(1).map_err(db("mi_empresa_rol"))?,
    }))
}

/// Serializa a las peticiones de la misma clave hasta el fin de la transacción.
pub async fn candado(
    txn: &impl ConnectionTrait,
    clave: &str,
    donde: &'static str,
) -> Result<(), ApiError> {
    txn.execute(stmt(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        vec![clave.to_string().into()],
    ))
    .await
    .map_err(db(donde))?;
    Ok(())
}

// ------------------------------------------------------------------ intentos

/// Intentos de `subject` de esta clase en la ventana. `None` si aún hay cupo; si no, los
/// segundos hasta que el más viejo salga de la ventana.
pub async fn espera_por_intentos(
    conn: &impl ConnectionTrait,
    kind: &str,
    subject: &str,
    ventana_secs: i64,
    maximo: i64,
) -> Result<Option<u64>, ApiError> {
    let fila = conn
        .query_one(stmt(
            "SELECT count(*), \
                    COALESCE(ceil(extract(epoch from \
                        (min(created_at) + make_interval(secs => $3) - now()))), 1)::bigint \
             FROM rate_events \
             WHERE kind = $1 AND subject_id = $2::uuid \
               AND created_at > now() - make_interval(secs => $3)",
            vec![
                kind.to_string().into(),
                subject.to_string().into(),
                (ventana_secs as f64).into(),
            ],
        ))
        .await
        .map_err(db("contar_intentos"))?
        .ok_or_else(|| interno("contar_intentos_fila"))?;
    let n: i64 = fila.try_get_by_index(0).map_err(db("contar_intentos_n"))?;
    let espera: i64 = fila.try_get_by_index(1).map_err(db("contar_intentos_t"))?;
    Ok((n >= maximo).then(|| espera.max(1) as u64))
}

pub async fn registrar_intento(
    conn: &impl ConnectionTrait,
    kind: &str,
    subject: &str,
) -> Result<(), ApiError> {
    conn.execute(stmt(
        "INSERT INTO rate_events (id, kind, subject_id) VALUES ($1::uuid, $2, $3::uuid)",
        vec![
            migration::new_id().to_string().into(),
            kind.to_string().into(),
            subject.to_string().into(),
        ],
    ))
    .await
    .map_err(db("registrar_intento"))?;
    Ok(())
}

// ------------------------------------------------------------------ auditoría

/// La IP de quien llama, solo para hashearla en `access_audit`: el primer salto de
/// `X-Forwarded-For` (el servicio va detrás de un proxy) o, si no hay, la dirección del socket.
/// Sirve para el rastro, no para decidir nada: falsearla no abre ningún acceso.
#[derive(Debug, Clone, Default)]
pub struct IpCliente(pub Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for IpCliente {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Infallible> {
        let reenviada = parts
            .headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|v| !v.is_empty() && v.len() <= 64)
            .map(str::to_string);
        let socket = || {
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0.ip().to_string())
        };
        Ok(Self(reenviada.or_else(socket)))
    }
}

/// Una fila de `access_audit` sobre una empresa. Ver [`auditar_recurso`].
pub async fn auditar(
    conn: &impl ConnectionTrait,
    pii: &PiiKeys,
    actor: &str,
    accion: &str,
    recurso: Option<&str>,
    ip: &IpCliente,
) -> Result<(), ApiError> {
    auditar_recurso(conn, pii, actor, accion, "company", recurso, ip).await
}

/// Una fila de `access_audit`: quién, qué, sobre qué recurso y la IP hasheada. **Sin contenido.**
/// Si falla, falla la operación que la llama (va en su misma transacción). `tipo` es una
/// constante del código (`company`, `job_offer`, `job_application`), nunca un dato recibido.
pub async fn auditar_recurso(
    conn: &impl ConnectionTrait,
    pii: &PiiKeys,
    actor: &str,
    accion: &str,
    tipo: &'static str,
    recurso: Option<&str>,
    ip: &IpCliente,
) -> Result<(), ApiError> {
    conn.execute(stmt(
        "INSERT INTO access_audit (id, actor_user_id, action, resource_type, resource_id, ip_hash) \
         VALUES ($1::uuid, $2::uuid, $3, $4, $5::uuid, $6)",
        vec![
            migration::new_id().to_string().into(),
            actor.to_string().into(),
            accion.to_string().into(),
            tipo.into(),
            recurso.map(str::to_string).into(),
            ip.0.as_deref().map(|i| pii.ip_hash(i)).into(),
        ],
    ))
    .await
    .map_err(db("auditar"))?;
    Ok(())
}

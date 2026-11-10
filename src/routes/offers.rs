//! `/api/v1/company/offers`: las ofertas de **mi** empresa (HU-50 y HU-51, Fase 5a, paso 4).
//!
//! Contrato: `api.md`, «Ofertas del reclutador». Reglas que atraviesan todas las rutas:
//!
//! - La empresa sale de la membresía de quien llama (`company_scope::company_of`), nunca del path
//!   ni del cuerpo. Toda consulta por id filtra también por `company_id`; una oferta ajena, una
//!   inexistente y un id mal formado dan el **mismo** 404 (SX6).
//! - Orden de validación: 401 → 403 `cuenta_no_disponible` → 403 `sin_permiso` (la tabla de rutas)
//!   → 413 → 422 del cuerpo o la consulta → 404 → 403 de la empresa → 409 de estado → 422
//!   `oferta_incompleta` → 429.
//! - Cada transición y cada edición lee la fila con `FOR UPDATE` dentro de una transacción: dos
//!   peticiones simultáneas se serializan y la segunda ve el estado nuevo.
//! - El texto de la oferta es dato no confiable: nada de él llega a un log ni a un mensaje de
//!   error, y los mensajes de error son textos fijos por causa.

use axum::{
    body::Bytes,
    extract::{rejection::BytesRejection, Path, RawQuery, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sea_orm::{ConnectionTrait, DbErr, TransactionTrait};
use serde_json::{json, Value};

use super::table::{Access, RouteSpec};
use crate::{
    auth_user::AuthUser,
    company_gate::{self, Gate},
    company_scope::{company_of, CompanyCtx},
    offers::{
        input::{self, InputError, Offer},
        store::{self, Stored},
    },
    state::AppState,
};

/// Tope del cuerpo de `POST`/`PUT` de ofertas.
pub const OFFER_BODY_LIMIT: usize = 64 * 1024;
/// Ofertas que una empresa puede crear por hora (SX5).
pub const CREATES_PER_HOUR: i64 = 20;
const DEFAULT_LIMIT: u64 = 20;
const MAX_LIMIT: u64 = 50;

pub fn routes() -> Vec<RouteSpec> {
    vec![
        RouteSpec {
            path: "/api/v1/company/offers",
            methods: &["POST"],
            access: Access::Permission("offer.create"),
            body_limit: Some(OFFER_BODY_LIMIT),
            handler: post(create),
        },
        RouteSpec {
            path: "/api/v1/company/offers",
            methods: &["GET"],
            access: Access::Permission("offer.update"),
            body_limit: None,
            handler: get(list),
        },
        RouteSpec {
            path: "/api/v1/company/offers/{id}",
            methods: &["GET", "PUT", "DELETE"],
            access: Access::Permission("offer.update"),
            body_limit: Some(OFFER_BODY_LIMIT),
            handler: get(read).put(replace).delete(remove),
        },
        RouteSpec {
            path: "/api/v1/company/offers/{id}/publish",
            methods: &["POST"],
            access: Access::Permission("offer.publish"),
            body_limit: None,
            handler: post(publish),
        },
        RouteSpec {
            path: "/api/v1/company/offers/{id}/pause",
            methods: &["POST"],
            access: Access::Permission("offer.publish"),
            body_limit: None,
            handler: post(pause),
        },
        RouteSpec {
            path: "/api/v1/company/offers/{id}/close",
            methods: &["POST"],
            access: Access::Permission("offer.close"),
            body_limit: None,
            handler: post(close),
        },
    ]
}

// ------------------------------------------------------------------ errores

enum ApiError {
    /// Recurso ajeno, inexistente, id mal formado o persona sin empresa: siempre el mismo cuerpo.
    NotFound,
    TooLarge,
    /// Textos fijos: jamás un dato recibido.
    Invalid(&'static str),
    UnknownField,
    UnknownParam,
    Incomplete(Vec<&'static str>),
    Forbidden(&'static str, &'static str),
    Conflict(&'static str, &'static str),
    TooMany(i64),
    Internal,
}

impl From<InputError> for ApiError {
    fn from(e: InputError) -> Self {
        match e {
            InputError::UnknownField => Self::UnknownField,
            InputError::Invalid(m) => Self::Invalid(m),
        }
    }
}

fn error(estado: StatusCode, codigo: &str, mensaje: &str) -> Response {
    (estado, Json(json!({"error": codigo, "message": mensaje}))).into_response()
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => error(
                StatusCode::NOT_FOUND,
                "no_encontrado",
                "No encontramos lo que buscas.",
            ),
            Self::TooLarge => error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "cuerpo_demasiado_grande",
                "El cuerpo de la petición es demasiado grande.",
            ),
            Self::Invalid(m) => error(StatusCode::UNPROCESSABLE_ENTITY, "datos_invalidos", m),
            Self::UnknownField => error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "campo_no_permitido",
                "El cuerpo trae un campo que no se admite.",
            ),
            Self::UnknownParam => error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "parametro_no_permitido",
                "La consulta trae un parámetro que no se admite.",
            ),
            Self::Incomplete(campos) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "error": "oferta_incompleta",
                    "message": "Faltan datos para publicar la oferta.",
                    "fields": campos,
                })),
            )
                .into_response(),
            Self::Forbidden(c, m) => error(StatusCode::FORBIDDEN, c, m),
            Self::Conflict(c, m) => error(StatusCode::CONFLICT, c, m),
            Self::TooMany(segundos) => (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, segundos.to_string())],
                Json(json!({
                    "error": "demasiados_intentos",
                    "message": "Tu empresa creó demasiadas ofertas en la última hora. Inténtalo más tarde.",
                })),
            )
                .into_response(),
            Self::Internal => error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "error_interno",
                "Algo salió mal de nuestro lado. Inténtalo de nuevo.",
            ),
        }
    }
}

/// Registra el fallo por categoría, **nunca** con el texto del error de la base (podría citar
/// valores del usuario).
fn db(donde: &'static str) -> impl Fn(DbErr) -> ApiError {
    move |_| {
        tracing::error!(donde, "error interno");
        ApiError::Internal
    }
}

const ESTADO_INVALIDO: ApiError = ApiError::Conflict(
    "estado_invalido",
    "La oferta no está en un estado que permita esto.",
);

// ------------------------------------------------------------------ comunes

/// Lee el cuerpo (un 413 propio, en JSON, en vez del texto plano de axum).
fn cuerpo(b: Result<Bytes, BytesRejection>) -> Result<Bytes, ApiError> {
    b.map_err(|e| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::TooLarge
        } else {
            ApiError::Invalid("El cuerpo no tiene el formato esperado.")
        }
    })
}

/// Id de path: UUID canónico o 404 (igual que uno inexistente).
fn id_de_path(id: &str) -> Result<String, ApiError> {
    input::uuid_canonico(id).ok_or(ApiError::NotFound)
}

async fn empresa(s: &AppState, user: &AuthUser) -> Result<CompanyCtx, ApiError> {
    company_of(&s.db, &user.id)
        .await
        .map_err(db("empresa_de_la_persona"))?
        .ok_or(ApiError::NotFound)
}

fn ymd(hoy: &str) -> Result<input::Ymd, ApiError> {
    input::fecha(hoy).ok_or_else(|| {
        tracing::error!(donde = "fecha_de_hoy", "error interno");
        ApiError::Internal
    })
}

/// Todos los `*_id` existen, están activos en su catálogo y la ciudad es del estado (OF11/OF12).
async fn validar_catalogos<C: ConnectionTrait>(c: &C, o: &Offer) -> Result<(), ApiError> {
    const MSG: &str = "Un dato del catálogo no existe o no está disponible.";
    let uno = [
        ("languages", &o.main_language_id),
        ("professions", &o.profession_id),
        ("employment_sectors", &o.employment_sector_id),
        ("job_hierarchies", &o.job_hierarchy_id),
        ("academic_levels", &o.minimum_academic_level_id),
        ("states", &o.state_id),
        ("salary_ranges", &o.salary_range_id),
        ("working_shifts", &o.working_shift_id),
        ("contract_types", &o.contract_type_id),
    ];
    for (tabla, id) in uno {
        if let Some(id) = id {
            if !store::all_active(c, tabla, std::slice::from_ref(id))
                .await
                .map_err(db("validar_catalogo"))?
            {
                return Err(ApiError::Invalid(MSG));
            }
        }
    }
    match (&o.city_id, &o.state_id) {
        (Some(ciudad), Some(estado)) => {
            if !store::city_in_state(c, ciudad, estado)
                .await
                .map_err(db("validar_ciudad"))?
            {
                return Err(ApiError::Invalid(MSG));
            }
        }
        (Some(_), None) => {
            return Err(ApiError::Invalid("La ciudad necesita su departamento."));
        }
        _ => {}
    }
    let skills: Vec<String> = o.skills.iter().map(|s| s.skill_id.clone()).collect();
    let idiomas: Vec<String> = o.languages.iter().map(|l| l.language_id.clone()).collect();
    for (tabla, ids) in [
        ("skills", &skills),
        ("languages", &idiomas),
        ("driver_licenses", &o.driver_license_ids),
    ] {
        if !store::all_active(c, tabla, ids)
            .await
            .map_err(db("validar_requisitos"))?
        {
            return Err(ApiError::Invalid(MSG));
        }
    }
    Ok(())
}

fn detalle(s: &Stored) -> Value {
    let o = &s.offer;
    json!({
        "id": s.id,
        "status": s.status,
        "title": o.title,
        "description": o.description,
        "is_confidential": o.is_confidential,
        "is_experience_required": o.is_experience_required,
        "required_experience": o.required_experience,
        "main_language_id": o.main_language_id,
        "schedule": o.schedule,
        "required_availability_to_travel": o.required_availability_to_travel,
        "required_driver_license": !o.driver_license_ids.is_empty(),
        "vacancies": o.vacancies,
        "hiring_date": o.hiring_date,
        "profession_id": o.profession_id,
        "employment_sector_id": o.employment_sector_id,
        "job_hierarchy_id": o.job_hierarchy_id,
        "minimum_academic_level_id": o.minimum_academic_level_id,
        "state_id": o.state_id,
        "city_id": o.city_id,
        "salary_range_id": o.salary_range_id,
        "working_shift_id": o.working_shift_id,
        "contract_type_id": o.contract_type_id,
        "skills": o.skills.iter().map(|k| json!({
            "skill_id": k.skill_id, "is_required": k.is_required, "min_level": k.min_level,
        })).collect::<Vec<_>>(),
        "languages": o.languages.iter().map(|l| json!({
            "language_id": l.language_id, "min_proficiency": l.min_proficiency,
        })).collect::<Vec<_>>(),
        "driver_license_ids": o.driver_license_ids,
        "publication_date": s.publication_date,
        "closed_at": s.closed_at,
        "created_at": s.created_at,
        "updated_at": s.updated_at,
        "applications_count": s.applications_count,
        "created_by": s.created_by.as_ref().map(|(id, nombre)| json!({"id": id, "display_name": nombre})),
    })
}

// ------------------------------------------------------------------ rutas

/// `POST /company/offers`: cuerpo → empresa → límite por hora → insertar.
async fn create(
    State(s): State<AppState>,
    user: AuthUser,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let bytes = cuerpo(body)?;
    let oferta = input::parse_offer(&bytes)?;
    if let Some(f) = &oferta.hiring_date {
        let hoy = store::today(&s.db).await.map_err(db("hoy"))?;
        input::rango_de_contratacion(f, ymd(&hoy)?)?;
    }
    validar_catalogos(&s.db, &oferta).await?;
    let ctx = empresa(&s, &user).await?;

    let txn = s.db.begin().await.map_err(db("crear_begin"))?;
    store::lock_company_creates(&txn, &ctx.company_id)
        .await
        .map_err(db("crear_candado"))?;
    let (recientes, espera) = store::creates_last_hour(&txn, &ctx.company_id)
        .await
        .map_err(db("crear_limite"))?;
    if recientes >= CREATES_PER_HOUR {
        return Err(ApiError::TooMany(espera));
    }
    let id = store::insert(&txn, &ctx.company_id, &ctx.recruiter_id, &oferta)
        .await
        .map_err(db("crear_insertar"))?;
    txn.commit().await.map_err(db("crear_commit"))?;

    let guardada = store::load(&s.db, &ctx.company_id, &id, false)
        .await
        .map_err(db("crear_leer"))?
        .ok_or(ApiError::Internal)?;
    Ok((StatusCode::CREATED, Json(detalle(&guardada))).into_response())
}

struct ListQuery {
    limit: u64,
    status: Option<String>,
    cursor: Option<(i64, String)>,
}

fn decodificar(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut fuera = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                fuera.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                fuera.push(b' ');
                i += 1;
            }
            c => {
                fuera.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(fuera).ok()
}

/// `?limit&cursor&status`: un parámetro desconocido → `parametro_no_permitido` (primero); un
/// valor inválido, vacío o repetido → `datos_invalidos`.
fn parse_query(raw: Option<String>) -> Result<ListQuery, ApiError> {
    const MSG: &str = "Un parámetro de la consulta no es válido.";
    let raw = raw.unwrap_or_default();
    let mut pares = Vec::new();
    for trozo in raw.split('&').filter(|t| !t.is_empty()) {
        let (k, v) = trozo.split_once('=').unwrap_or((trozo, ""));
        let k = decodificar(k).ok_or(ApiError::Invalid(MSG))?;
        let v = decodificar(v).ok_or(ApiError::Invalid(MSG))?;
        pares.push((k, v));
    }
    if pares
        .iter()
        .any(|(k, _)| !["limit", "cursor", "status"].contains(&k.as_str()))
    {
        return Err(ApiError::UnknownParam);
    }
    let mut q = ListQuery {
        limit: DEFAULT_LIMIT,
        status: None,
        cursor: None,
    };
    let (mut vl, mut vc, mut vs) = (false, false, false);
    for (k, v) in pares {
        match k.as_str() {
            "limit" if !vl => {
                vl = true;
                let n: u64 = if v.bytes().all(|c| c.is_ascii_digit()) && !v.is_empty() {
                    v.parse().map_err(|_| ApiError::Invalid(MSG))?
                } else {
                    return Err(ApiError::Invalid(MSG));
                };
                if !(1..=MAX_LIMIT).contains(&n) {
                    return Err(ApiError::Invalid(MSG));
                }
                q.limit = n;
            }
            "status" if !vs => {
                vs = true;
                if !["draft", "published", "paused", "closed"].contains(&v.as_str()) {
                    return Err(ApiError::Invalid(MSG));
                }
                q.status = Some(v);
            }
            "cursor" if !vc => {
                vc = true;
                q.cursor = Some(cursor_de(&v).ok_or(ApiError::Invalid(MSG))?);
            }
            _ => return Err(ApiError::Invalid(MSG)),
        }
    }
    Ok(q)
}

/// El cursor es opaco para el cliente: `base64url("<micros>:<uuid>")`.
fn cursor_de(c: &str) -> Option<(i64, String)> {
    let bytes = URL_SAFE_NO_PAD.decode(c).ok()?;
    let texto = String::from_utf8(bytes).ok()?;
    let (micros, id) = texto.split_once(':')?;
    Some((micros.parse().ok()?, input::uuid_canonico(id)?))
}

fn cursor_a(micros: i64, id: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("{micros}:{id}"))
}

/// `GET /company/offers`: un número fijo de consultas (la empresa y una página).
async fn list(
    State(s): State<AppState>,
    user: AuthUser,
    RawQuery(raw): RawQuery,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    cuerpo(body)?;
    let q = parse_query(raw)?;
    let ctx = empresa(&s, &user).await?;
    let mut filas = store::list(
        &s.db,
        &ctx.company_id,
        q.status.as_deref(),
        q.cursor,
        q.limit,
    )
    .await
    .map_err(db("listar"))?;
    let hay_mas = filas.len() as u64 > q.limit;
    filas.truncate(q.limit as usize);
    let siguiente = if hay_mas {
        filas.last().map(|f| cursor_a(f.created_micros, &f.id))
    } else {
        None
    };
    let items: Vec<Value> = filas
        .iter()
        .map(|f| {
            json!({
                "id": f.id, "title": f.title, "status": f.status,
                "is_confidential": f.is_confidential, "vacancies": f.vacancies,
                "publication_date": f.publication_date,
                "applications_count": f.applications_count, "created_at": f.created_at,
            })
        })
        .collect();
    Ok(Json(json!({"items": items, "next_cursor": siguiente})).into_response())
}

/// `GET /company/offers/{id}`.
async fn read(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    cuerpo(body)?;
    let id = id_de_path(&id)?;
    let ctx = empresa(&s, &user).await?;
    let oferta = store::load(&s.db, &ctx.company_id, &id, false)
        .await
        .map_err(db("leer"))?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(detalle(&oferta)).into_response())
}

/// `PUT /company/offers/{id}`: reemplazo completo. Orden: cuerpo (422) → 404 → 409 `estado_invalido`
/// (cerrada) → 409 `confidencial_inmutable` → 409 `oferta_con_aplicaciones` → 422 de la fecha.
async fn replace(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let bytes = cuerpo(body)?;
    let nueva = input::parse_offer(&bytes)?;
    validar_catalogos(&s.db, &nueva).await?;
    let id = id_de_path(&id)?;
    let ctx = empresa(&s, &user).await?;

    let txn = s.db.begin().await.map_err(db("editar_begin"))?;
    let actual = store::load(&txn, &ctx.company_id, &id, true)
        .await
        .map_err(db("editar_leer"))?
        .ok_or(ApiError::NotFound)?;
    if actual.status == "closed" {
        return Err(ESTADO_INVALIDO);
    }
    // Congelado desde la primera publicación: quien la publicó fijó si se ve la empresa.
    if actual.publication_date.is_some() && actual.offer.is_confidential != nueva.is_confidential {
        return Err(ApiError::Conflict(
            "confidencial_inmutable",
            "Una oferta ya publicada no cambia si es confidencial.",
        ));
    }
    // Congelado desde la primera aplicación: todo salvo cuatro campos.
    if actual.applications_count > 0 && input::frozen_changed(&actual.offer, &nueva) {
        return Err(ApiError::Conflict(
            "oferta_con_aplicaciones",
            "La oferta ya tiene aplicaciones: solo puedes cambiar la descripción, el horario, \
             las vacantes y la fecha de contratación.",
        ));
    }
    // La fecha solo se valida si cambia: reenviar la misma (aunque ya venció) es idempotente.
    if nueva.hiring_date != actual.offer.hiring_date {
        if let Some(f) = &nueva.hiring_date {
            let hoy = store::today(&txn).await.map_err(db("hoy"))?;
            input::rango_de_contratacion(f, ymd(&hoy)?)?;
        }
    }
    store::replace(&txn, &ctx.company_id, &id, &nueva)
        .await
        .map_err(db("editar_guardar"))?;
    txn.commit().await.map_err(db("editar_commit"))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `DELETE /company/offers/{id}`: solo un borrador sin aplicaciones.
async fn remove(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    cuerpo(body)?;
    let id = id_de_path(&id)?;
    let ctx = empresa(&s, &user).await?;
    let txn = s.db.begin().await.map_err(db("borrar_begin"))?;
    let actual = store::load(&txn, &ctx.company_id, &id, true)
        .await
        .map_err(db("borrar_leer"))?
        .ok_or(ApiError::NotFound)?;
    if actual.status != "draft" || actual.applications_count > 0 {
        return Err(ESTADO_INVALIDO);
    }
    store::delete(&txn, &ctx.company_id, &id)
        .await
        .map_err(db("borrar"))?;
    txn.commit().await.map_err(db("borrar_commit"))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST .../publish`: 404 → 409 → 403 `empresa_no_verificada` → 403 `membresia_vencida` →
/// 422 `oferta_incompleta`. Sirve para publicar un borrador y para reanudar una pausada.
async fn publish(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    cuerpo(body)?;
    let id = id_de_path(&id)?;
    let ctx = empresa(&s, &user).await?;
    let txn = s.db.begin().await.map_err(db("publicar_begin"))?;
    let actual = store::load(&txn, &ctx.company_id, &id, true)
        .await
        .map_err(db("publicar_leer"))?
        .ok_or(ApiError::NotFound)?;
    if actual.status != "draft" && actual.status != "paused" {
        return Err(ESTADO_INVALIDO);
    }
    let company = uuid::Uuid::parse_str(&ctx.company_id).map_err(|_| ApiError::Internal)?;
    match company_gate::check(&txn, company)
        .await
        .map_err(db("publicar_gate"))?
    {
        Ok(()) => {}
        Err(Gate::NotVerified) => {
            return Err(ApiError::Forbidden(
                "empresa_no_verificada",
                "Tu empresa todavía no está verificada.",
            ))
        }
        Err(Gate::MembershipExpired) => {
            return Err(ApiError::Forbidden(
                "membresia_vencida",
                "La membresía de tu empresa no está vigente.",
            ))
        }
    }
    let hoy = store::today(&txn).await.map_err(db("hoy"))?;
    let faltan = input::missing_for_publish(&actual.offer, ymd(&hoy)?);
    if !faltan.is_empty() {
        return Err(ApiError::Incomplete(faltan));
    }
    let fecha = store::set_status(&txn, &ctx.company_id, &id, "published")
        .await
        .map_err(db("publicar"))?;
    txn.commit().await.map_err(db("publicar_commit"))?;
    Ok(Json(json!({"id": id, "status": "published", "publication_date": fecha})).into_response())
}

/// Pausar (`published → paused`) y cerrar (`published|paused → closed`): sin comprobar la
/// verificación ni la membresía; cerrar fija `closed_at`.
async fn transicion(
    s: &AppState,
    user: &AuthUser,
    id: &str,
    desde: &[&str],
    hacia: &'static str,
) -> Result<Response, ApiError> {
    let id = id_de_path(id)?;
    let ctx = empresa(s, user).await?;
    let txn = s.db.begin().await.map_err(db("transicion_begin"))?;
    let actual = store::load(&txn, &ctx.company_id, &id, true)
        .await
        .map_err(db("transicion_leer"))?
        .ok_or(ApiError::NotFound)?;
    if !desde.contains(&actual.status.as_str()) {
        return Err(ESTADO_INVALIDO);
    }
    store::set_status(&txn, &ctx.company_id, &id, hacia)
        .await
        .map_err(db("transicion"))?;
    txn.commit().await.map_err(db("transicion_commit"))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn pause(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    cuerpo(body)?;
    transicion(&s, &user, &id, &["published"], "paused").await
}

async fn close(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    cuerpo(body)?;
    transicion(&s, &user, &id, &["published", "paused"], "closed").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_cursor_va_y_vuelve() {
        let id = "019a0000-0000-7000-8000-000000000001";
        let c = cursor_a(1_700_000_000_123_456, id);
        assert_eq!(cursor_de(&c), Some((1_700_000_000_123_456, id.to_string())));
        for malo in [
            "",
            "basura",
            "!!!",
            &URL_SAFE_NO_PAD.encode("x:y"),
            &URL_SAFE_NO_PAD.encode("5"),
        ] {
            assert_eq!(cursor_de(malo), None, "{malo}");
        }
    }

    #[test]
    fn la_consulta_se_valida() {
        let ok = |q: &str| parse_query(Some(q.to_string())).is_ok();
        assert!(ok(""));
        assert!(ok("limit=50&status=draft"));
        assert!(!ok("limit=0"));
        assert!(!ok("limit=51"));
        assert!(!ok("limit=-1"));
        assert!(!ok("limit="));
        assert!(!ok("status=otro"));
        assert!(!ok("limit=1&limit=2"));
        assert!(matches!(
            parse_query(Some("age=30&limit=x".into())),
            Err(ApiError::UnknownParam)
        ));
        assert!(matches!(
            parse_query(Some("limit=%zz".into())),
            Err(ApiError::Invalid(_))
        ));
        assert_eq!(decodificar("a%20b+c").as_deref(), Some("a b c"));
        assert_eq!(decodificar("%ff"), None);
    }
}

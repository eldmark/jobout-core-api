//! Aplicar, ver y retirar (candidato), Fase 5a, paso 6 (HU-52, HU-55).
//! Contrato: `api.md`, «Aplicar, ver y retirar (candidato)».
//!
//! Es la primera ruta por la que los datos de una persona pueden llegar a otra, así que:
//!
//! - El consentimiento se comprueba **primero** y no hay valor por defecto (`input::parse_apply`).
//! - Aquí no se copia ni se lee nada del perfil: se guardan **nombres** de campos y el panel
//!   (paso 7) lee el perfil vivo con `applications::shared_view`, que solo toca lo marcado.
//! - Todo filtra por dueño: una aplicación ajena, inexistente o con id mal formado es el mismo 404.
//! - Los errores son textos fijos: jamás repiten lo recibido ni nombran a la empresa de una
//!   oferta confidencial. Nada de lo compartido llega a un log.
//!
//! Orden al aplicar: 401 → 413 → cuerpo (422) → consentimiento (422) → `shared_fields` (422) → la
//! oferta existe y es visible (404) → es de mi empresa (409 `oferta_propia`) → ya apliqué (409
//! `ya_aplicaste`) → límite (429).

use axum::{
    body::Bytes,
    extract::{rejection::BytesRejection, Path, RawQuery, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json,
};
use sea_orm::{DbErr, SqlErr, TransactionTrait};
use serde_json::{json, Value};

use super::company_support::{codificar_cursor, parse_consulta, Pagina};
use super::table::{Access, RouteSpec};
use crate::{
    applications::{
        input::{parse_apply, ApplyError},
        move_status,
        store::{self, AppRow},
        MoveError, Scope, Status,
    },
    auth_user::AuthUser,
    offers::input::uuid_canonico,
    state::AppState,
};

/// Aplicaciones que una persona puede crear en 24 horas móviles (SX5).
pub const APPLIES_PER_DAY: i64 = 20;

pub fn routes() -> Vec<RouteSpec> {
    vec![
        RouteSpec {
            path: "/api/v1/offers/{id}/applications",
            methods: &["POST"],
            access: Access::Authenticated,
            body_limit: None,
            handler: post(apply),
        },
        RouteSpec {
            path: "/api/v1/me/applications",
            methods: &["GET"],
            access: Access::Authenticated,
            body_limit: None,
            handler: get(list),
        },
        RouteSpec {
            path: "/api/v1/me/applications/{id}",
            methods: &["GET"],
            access: Access::Authenticated,
            body_limit: None,
            handler: get(read),
        },
        RouteSpec {
            path: "/api/v1/me/applications/{id}/withdraw",
            methods: &["POST"],
            access: Access::Authenticated,
            body_limit: None,
            handler: post(withdraw),
        },
    ]
}

// ------------------------------------------------------------------ errores

pub enum ApiError {
    /// Ajeno, inexistente, no visible o id mal formado: siempre el mismo cuerpo.
    NotFound,
    TooLarge,
    Invalid(&'static str),
    Consent,
    UnknownField,
    UnknownParam,
    /// 409 con código y texto fijos.
    Conflict(&'static str, &'static str),
    /// 409 `ya_aplicaste` con el id de la aplicación (suya).
    AlreadyApplied(String),
    /// 409 `estado_invalido` con el estado vigente.
    BadState(Status),
    TooMany(i64),
    Internal,
}

fn error(estado: StatusCode, codigo: &str, mensaje: &str) -> Response {
    (estado, Json(json!({"error": codigo, "message": mensaje}))).into_response()
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => error(StatusCode::NOT_FOUND, "no_encontrado", "No encontramos lo que buscas."),
            Self::TooLarge => error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "cuerpo_demasiado_grande",
                "El cuerpo de la petición es demasiado grande.",
            ),
            Self::Invalid(m) => error(StatusCode::UNPROCESSABLE_ENTITY, "datos_invalidos", m),
            Self::Consent => error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "consentimiento_requerido",
                "Para aplicar tienes que aceptar compartir tus datos con la empresa.",
            ),
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
            Self::Conflict(c, m) => error(StatusCode::CONFLICT, c, m),
            Self::AlreadyApplied(id) => (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "ya_aplicaste",
                    "message": "Ya aplicaste a esta oferta.",
                    "application_id": id,
                })),
            )
                .into_response(),
            Self::BadState(estado) => (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "estado_invalido",
                    "message": "La aplicación no está en un estado que permita esto.",
                    "status": estado.as_str(),
                })),
            )
                .into_response(),
            Self::TooMany(segundos) => (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, segundos.max(1).to_string())],
                Json(json!({
                    "error": "demasiados_intentos",
                    "message": "Aplicaste a demasiadas ofertas en las últimas 24 horas. Inténtalo más tarde.",
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

/// Registra el fallo por categoría, **nunca** con el texto de la base (podría citar datos).
pub fn db(donde: &'static str) -> impl Fn(DbErr) -> ApiError {
    move |_| {
        tracing::error!(donde, "error interno");
        ApiError::Internal
    }
}

/// Lee el cuerpo (un 413 propio, en JSON).
pub fn cuerpo(b: Result<Bytes, BytesRejection>) -> Result<Bytes, ApiError> {
    b.map_err(|e| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::TooLarge
        } else {
            ApiError::Invalid("El cuerpo no tiene el formato esperado.")
        }
    })
}

fn id_de_path(id: &str) -> Result<String, ApiError> {
    uuid_canonico(id).ok_or(ApiError::NotFound)
}

impl From<super::company_support::ApiError> for ApiError {
    fn from(e: super::company_support::ApiError) -> Self {
        use super::company_support::ApiError as E;
        match e {
            E::NotFound => Self::NotFound,
            E::ParamNotAllowed => Self::UnknownParam,
            E::Invalid(m) => Self::Invalid(m),
            _ => Self::Internal,
        }
    }
}

// ------------------------------------------------------------------ salida

fn cat(c: &Option<store::Cat>) -> Value {
    c.as_ref().map_or(
        Value::Null,
        |c| json!({"id": c.id, "code": c.code, "name": c.name}),
    )
}

fn offer_json(r: &AppRow) -> Value {
    json!({
        "id": r.offer_id,
        "title": r.offer_title,
        "status": r.offer_status,
        "confidential": r.confidential,
        // Solo si no es confidencial o ya se reveló a esta aplicación.
        "company": r.company.as_ref().map(|(id, name, web)| json!({"id": id, "name": name, "website": web})),
        "state": cat(&r.state),
        "city": cat(&r.city),
    })
}

fn summary(r: &AppRow) -> Value {
    json!({
        "id": r.id,
        "status": r.status,
        "applied_at": r.applied_at,
        "status_changed_at": r.status_changed_at,
        "consented_at": r.consented_at,
        "offer": offer_json(r),
    })
}

async fn detail(s: &AppState, user_id: &str, id: &str) -> Result<Value, ApiError> {
    let fila = store::list_mine(&s.db, user_id, Some(id), None, None, 1)
        .await
        .map_err(db("leer_aplicacion"))?
        .into_iter()
        .next()
        .ok_or(ApiError::NotFound)?;
    let historial = store::history_of(&s.db, &fila.id)
        .await
        .map_err(db("leer_historial"))?;
    let mut v = summary(&fila);
    v["shared_fields"] = json!(fila.shared_fields);
    // Solo `{status, at}`: nunca quién decidió ni la nota interna.
    v["history"] = json!(historial
        .iter()
        .map(|(status, at)| json!({"status": status, "at": at}))
        .collect::<Vec<_>>());
    Ok(v)
}

// ------------------------------------------------------------------ rutas

async fn apply(
    State(s): State<AppState>,
    user: AuthUser,
    Path(offer): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let bytes = cuerpo(body)?;
    let entrada = parse_apply(&bytes).map_err(|e| match e {
        ApplyError::Consent => ApiError::Consent,
        ApplyError::UnknownField => ApiError::UnknownField,
        ApplyError::Invalid(m) => ApiError::Invalid(m),
    })?;
    let offer = id_de_path(&offer)?;

    let txn = s.db.begin().await.map_err(db("aplicar_begin"))?;
    // Una persona a la vez: el duplicado y el tope se deciden sobre filas ya confirmadas.
    store::lock_applicant(&txn, &user.id)
        .await
        .map_err(db("aplicar_candado"))?;
    let applicant = store::applicant_of(&txn, &user.id)
        .await
        .map_err(db("aplicar_persona"))?
        .ok_or(ApiError::NotFound)?;
    let company = store::visible_offer_company(&txn, &offer)
        .await
        .map_err(db("aplicar_oferta"))?
        .ok_or(ApiError::NotFound)?;
    if store::is_member(&txn, &user.id, &company)
        .await
        .map_err(db("aplicar_membresia"))?
    {
        return Err(ApiError::Conflict(
            "oferta_propia",
            "No puedes aplicar a una oferta de tu propia empresa.",
        ));
    }
    if let Some(previa) = store::existing(&txn, &applicant, &offer)
        .await
        .map_err(db("aplicar_previa"))?
    {
        return Err(ApiError::AlreadyApplied(previa));
    }
    let (n, espera) = store::last_day(&txn, &applicant, APPLIES_PER_DAY)
        .await
        .map_err(db("aplicar_limite"))?;
    if n >= APPLIES_PER_DAY {
        return Err(ApiError::TooMany(espera));
    }
    let id = match store::insert(&txn, &applicant, &user.id, &offer, &entrada.shared_fields).await {
        Ok(id) => id,
        // La restricción única es la última palabra si algo se coló.
        Err(e) if matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => {
            drop(txn);
            let previa = store::existing(&s.db, &applicant, &offer)
                .await
                .map_err(db("aplicar_previa_tras_choque"))?
                .ok_or(ApiError::Internal)?;
            return Err(ApiError::AlreadyApplied(previa));
        }
        Err(_) => {
            tracing::error!(donde = "aplicar_insertar", "error interno");
            return Err(ApiError::Internal);
        }
    };
    txn.commit().await.map_err(db("aplicar_commit"))?;
    let v = detail(&s, &user.id, &id).await?;
    Ok((StatusCode::CREATED, Json(v)).into_response())
}

async fn list(
    State(s): State<AppState>,
    user: AuthUser,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let q = parse_consulta(raw.as_deref(), &["limit", "cursor", "status"])?;
    let pagina = Pagina::de(&q)?;
    let estado = match q.get("status") {
        None => None,
        Some(e) => Some(Status::parse(e).ok_or(ApiError::Invalid("El estado no es válido."))?),
    };
    let mut filas = store::list_mine(
        &s.db,
        &user.id,
        None,
        estado.map(Status::as_str),
        pagina.cursor,
        pagina.limite,
    )
    .await
    .map_err(db("listar_aplicaciones"))?;
    let hay_mas = filas.len() as i64 > pagina.limite;
    filas.truncate(pagina.limite as usize);
    let siguiente = if hay_mas {
        filas
            .last()
            .map(|f| codificar_cursor(f.applied_micros, &f.id))
    } else {
        None
    };
    Ok(Json(json!({
        "items": filas.iter().map(summary).collect::<Vec<_>>(),
        "next_cursor": siguiente,
    }))
    .into_response())
}

async fn read(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = id_de_path(&id)?;
    Ok(Json(detail(&s, &user.id, &id).await?).into_response())
}

async fn withdraw(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = id_de_path(&id)?;
    match move_status(
        &s.db,
        &s.pii,
        Scope::Applicant { user_id: &user.id },
        &id,
        Status::Withdrawn,
        None,
    )
    .await
    {
        Ok(_) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(MoveError::NotFound) => Err(ApiError::NotFound),
        Err(MoveError::Invalid(actual)) => Err(ApiError::BadState(actual)),
        Err(MoveError::Crypto) | Err(MoveError::Db(_)) => {
            tracing::error!(donde = "retirar", "error interno");
            Err(ApiError::Internal)
        }
    }
}

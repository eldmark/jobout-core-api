//! Sesiones de entrevista (Fase 3, paso 6): crear, listar, leer, pausar, retomar, abandonar y
//! borrar. Contrato en `documentation/api.md`, sección «Entrevista (Fase 3)».
//!
//! - Todas las rutas son `Authenticated` y filtran por dueño: la sesión de otra persona, la que no
//!   existe y un id que no es UUID dan el mismo 404 `no_encontrado`.
//! - El texto de los turnos se guarda siempre cifrado (`interview_store::insert_turn`) y se
//!   descifra solo al leerlo; ni un error ni un log lo llevan.
//! - La cuota mensual del plan (`limite_del_plan`) es la última validación de `POST /interviews` y
//!   se aplica dentro de la transacción que crea la sesión (`crate::quota`).
//! - Quedan fuera (pasos siguientes): turnos, `finish` y `score`.

use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{rejection::QueryRejection, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json,
};
use serde_json::{json, Value};
use uuid::Uuid;

use super::table::{Access, RouteSpec};
use crate::{
    auth_user::AuthUser,
    interview::{DeterministicEngine, InterviewEngine, Level, RecruiterCode, RecruiterConfig},
    interview_store::{
        self as store, CreateOutcome, Cursor, ResumeOutcome, StoreError, Transition,
        TransitionOutcome,
    },
    quota::QuotaPolicy,
    state::AppState,
};

/// Todas protegidas: la tabla de rutas aplica la autenticación al montarlas.
pub fn routes() -> Vec<RouteSpec> {
    vec![
        RouteSpec {
            path: "/api/v1/interviews",
            methods: &["GET", "POST"],
            access: Access::Authenticated,
            body_limit: None,
            handler: get(list).post(create),
        },
        RouteSpec {
            path: "/api/v1/interviews/{id}",
            methods: &["GET", "DELETE"],
            access: Access::Authenticated,
            body_limit: None,
            handler: get(read).delete(remove),
        },
        RouteSpec {
            path: "/api/v1/interviews/{id}/pause",
            methods: &["POST"],
            access: Access::Authenticated,
            body_limit: None,
            handler: post(pause),
        },
        RouteSpec {
            path: "/api/v1/interviews/{id}/resume",
            methods: &["POST"],
            access: Access::Authenticated,
            body_limit: None,
            handler: post(resume),
        },
        RouteSpec {
            path: "/api/v1/interviews/{id}/abandon",
            methods: &["POST"],
            access: Access::Authenticated,
            body_limit: None,
            handler: post(abandon),
        },
    ]
}

enum ApiError {
    Invalid,
    NotFound,
    Locked,
    Unavailable,
    SessionOpen(Uuid),
    PlanLimit(String),
    BadState,
    Engine,
    Internal,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (estado, codigo, mensaje, extra) = match self {
            Self::Invalid => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "datos_invalidos",
                "Revisa los datos enviados.",
                None,
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "no_encontrado",
                "No encontramos lo que buscas.",
                None,
            ),
            Self::Locked => (
                StatusCode::FORBIDDEN,
                "nivel_bloqueado",
                "Aún no desbloqueas este nivel.",
                None,
            ),
            Self::Unavailable => (
                StatusCode::CONFLICT,
                "nivel_no_disponible",
                "Este nivel todavía no está disponible.",
                None,
            ),
            Self::SessionOpen(id) => (
                StatusCode::CONFLICT,
                "sesion_en_curso",
                "Ya tienes una entrevista en curso. Termínala o abandónala para empezar otra.",
                Some(("session_id", json!(id))),
            ),
            Self::PlanLimit(renews_at) => (
                StatusCode::FORBIDDEN,
                "limite_del_plan",
                "Ya usaste las entrevistas de tu plan este mes. Se renuevan el primer día del mes siguiente.",
                Some(("renews_at", json!(renews_at))),
            ),
            Self::BadState => (
                StatusCode::CONFLICT,
                "estado_invalido",
                "La entrevista no está en un estado que permita esto.",
                None,
            ),
            Self::Engine => (
                StatusCode::SERVICE_UNAVAILABLE,
                "motor_no_disponible",
                "El recruiter no está disponible ahora. Inténtalo en un momento.",
                None,
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "error_interno",
                "Algo salió mal de nuestro lado. Inténtalo de nuevo.",
                None,
            ),
        };
        let mut cuerpo = json!({"error": codigo, "message": mensaje});
        if let Some((campo, valor)) = extra {
            cuerpo[campo] = valor;
        }
        (estado, Json(cuerpo)).into_response()
    }
}

/// Registra el fallo por categoría, **nunca** con el texto del error ni de un turno.
fn interno(donde: &'static str) -> impl Fn(StoreError) -> ApiError {
    move |_| {
        tracing::error!(donde, "error interno");
        ApiError::Internal
    }
}

/// Un id de la ruta: si no es un UUID, es un 404 como cualquier otro inexistente.
fn id_de_ruta(texto: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(texto).map_err(|_| ApiError::NotFound)
}

async fn applicant_de(state: &AppState, user: &AuthUser) -> Result<Uuid, ApiError> {
    store::applicant_id(&state.db, &user.id)
        .await
        .map_err(interno("leer_applicant"))?
        .ok_or_else(|| {
            tracing::error!(donde = "sin_applicant", "error interno");
            ApiError::Internal
        })
}

// ------------------------------------------------------------------ POST /interviews

/// El cuerpo de `POST /interviews`: dos UUID. Cualquier otra cosa es 422, sin repetir nada.
fn leer_cuerpo(cuerpo: &[u8]) -> Result<(Uuid, Uuid), ApiError> {
    let json: Value = serde_json::from_slice(cuerpo).map_err(|_| ApiError::Invalid)?;
    let id = |campo: &str| {
        json.get(campo)
            .and_then(Value::as_str)
            .and_then(|t| Uuid::parse_str(t).ok())
            .ok_or(ApiError::Invalid)
    };
    Ok((id("recruiter_id")?, id("level_id")?))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    cuerpo: Bytes,
) -> Result<Response, ApiError> {
    // Orden del contrato: cuerpo -> existen -> disponible -> desbloqueado -> sin sesión activa -> cuota
    // (esta última, dentro de `create_session`).
    let (recruiter_id, level_id) = leer_cuerpo(&cuerpo)?;
    let applicant = applicant_de(&state, &user).await?;

    let recruiter = store::find_recruiter(&state.db, recruiter_id)
        .await
        .map_err(interno("leer_recruiter"))?
        .ok_or(ApiError::NotFound)?;
    let level = store::find_level(&state.db, applicant, level_id)
        .await
        .map_err(interno("leer_nivel"))?
        .ok_or(ApiError::NotFound)?;
    if !level.available {
        return Err(ApiError::Unavailable);
    }
    if !level.unlocked {
        return Err(ApiError::Locked);
    }
    if let Some(session_id) = store::open_session_id(&state.db, applicant)
        .await
        .map_err(interno("leer_sesion_abierta"))?
    {
        return Err(ApiError::SessionOpen(session_id));
    }

    // La primera pregunta sale del motor: el historial vacío da la apertura del recruiter.
    let config = RecruiterConfig {
        recruiter: RecruiterCode::from_code(&recruiter.code).ok_or(ApiError::Engine)?,
        level: Level::from_code(&level.code).ok_or(ApiError::Engine)?,
    };
    let primera = DeterministicEngine
        .next_turn(&config, &[])
        .map_err(|_| ApiError::Engine)?;

    // Dos creaciones simultáneas pasan la comprobación de arriba: el índice único parcial decide.
    let session_id = match store::create_session(
        &state.db,
        &state.pii,
        applicant,
        recruiter.id,
        level.id,
        &primera.message,
        QuotaPolicy::current(),
    )
    .await
    .map_err(interno("crear_sesion"))?
    {
        CreateOutcome::Created { session_id } => session_id,
        CreateOutcome::OpenExists { session_id } => return Err(ApiError::SessionOpen(session_id)),
        CreateOutcome::LimitReached { renews_at } => return Err(ApiError::PlanLimit(renews_at)),
    };

    let entrevista = store::get_session(&state.db, &state.pii, applicant, session_id)
        .await
        .map_err(interno("leer_sesion_creada"))?
        .ok_or_else(|| interno("sesion_creada_ausente")(StoreError::Corrupt))?;
    Ok((StatusCode::CREATED, Json(entrevista)).into_response())
}

// ------------------------------------------------------------------ GET /interviews

const LIMITE_DEFECTO: u64 = 20;
const LIMITE_MAXIMO: u64 = 50;
const ESTADOS: [&str; 4] = ["in_progress", "paused", "completed", "abandoned"];

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    query: Result<Query<HashMap<String, String>>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = query.map_err(|_| ApiError::Invalid)?;
    let limit = match q.get("limit") {
        None => LIMITE_DEFECTO,
        Some(t) => t
            .parse::<u64>()
            .ok()
            .filter(|n| (1..=LIMITE_MAXIMO).contains(n))
            .ok_or(ApiError::Invalid)?,
    };
    let status = match q.get("status") {
        None => None,
        Some(t) => Some(
            ESTADOS
                .into_iter()
                .find(|e| e == t)
                .ok_or(ApiError::Invalid)?,
        ),
    };
    let cursor = match q.get("cursor") {
        None => None,
        Some(t) => Some(Cursor::decode(t).ok_or(ApiError::Invalid)?),
    };

    let applicant = applicant_de(&state, &user).await?;
    let (items, siguiente) = store::list_sessions(&state.db, applicant, status, limit, cursor)
        .await
        .map_err(interno("listar_sesiones"))?;
    Ok(Json(json!({
        "items": items,
        "next_cursor": siguiente.map(|c| c.encode()),
    })))
}

// ------------------------------------------------------------------ por id

async fn read(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<store::Interview>, ApiError> {
    let id = id_de_ruta(&id)?;
    let applicant = applicant_de(&state, &user).await?;
    store::get_session(&state.db, &state.pii, applicant, id)
        .await
        .map_err(interno("leer_sesion"))?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn cambiar(
    state: &AppState,
    user: &AuthUser,
    id: &str,
    cambio: Transition,
    donde: &'static str,
) -> Result<StatusCode, ApiError> {
    let id = id_de_ruta(id)?;
    let applicant = applicant_de(state, user).await?;
    match store::transition(&state.db, applicant, id, cambio)
        .await
        .map_err(interno(donde))?
    {
        TransitionOutcome::Done => Ok(StatusCode::NO_CONTENT),
        TransitionOutcome::NotFound => Err(ApiError::NotFound),
        TransitionOutcome::InvalidState => Err(ApiError::BadState),
    }
}

async fn pause(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    cambiar(&state, &user, &id, Transition::Pause, "pausar_sesion").await
}

async fn abandon(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    cambiar(&state, &user, &id, Transition::Abandon, "abandonar_sesion").await
}

async fn resume(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id = id_de_ruta(&id)?;
    let applicant = applicant_de(&state, &user).await?;
    match store::resume_session(&state.db, &state.pii, applicant, id)
        .await
        .map_err(interno("retomar_sesion"))?
    {
        ResumeOutcome::Resumed(turno) => Ok(Json(json!({ "turn": turno }))),
        ResumeOutcome::NotFound => Err(ApiError::NotFound),
        ResumeOutcome::InvalidState => Err(ApiError::BadState),
    }
}

async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = id_de_ruta(&id)?;
    let applicant = applicant_de(&state, &user).await?;
    if store::delete_session(&state.db, applicant, id)
        .await
        .map_err(interno("borrar_sesion"))?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

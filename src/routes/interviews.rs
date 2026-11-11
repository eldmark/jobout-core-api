//! Sesiones de entrevista (Fase 3, paso 6): crear, listar, leer, pausar, retomar, abandonar y
//! borrar. Contrato en `documentation/api.md`, sección «Entrevista (Fase 3)».
//!
//! - Todas las rutas son `Authenticated` y filtran por dueño: la sesión de otra persona, la que no
//!   existe y un id que no es UUID dan el mismo 404 `no_encontrado`.
//! - El texto de los turnos se guarda siempre cifrado (`interview_store::insert_turn`) y se
//!   descifra solo al leerlo; ni un error ni un log lo llevan.
//! - La cuota mensual del plan (`limite_del_plan`) es la última validación de `POST /interviews` y
//!   se aplica dentro de la transacción que crea la sesión (`crate::quota`).
//! - Los turnos (`POST .../turns`, paso 7) guardan la respuesta y la réplica del recruiter en una
//!   transacción con la fila de la sesión bloqueada (`interview_store::answer_turn`).
//! - `finish` y `score` (paso 8) cierran y puntúan con la misma función que usa el cierre por turno.

use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{rejection::QueryRejection, Path, Query, State},
    http::{header, StatusCode},
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
        self as store, AnswerOutcome, CreateOutcome, Cursor, FinishOutcome, ResumeOutcome,
        ScoreOutcome, StoreError, Transition, TransitionOutcome,
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
        RouteSpec {
            path: "/api/v1/interviews/{id}/turns",
            methods: &["POST"],
            access: Access::Authenticated,
            // 4000 caracteres Unicode (hasta 16 000 bytes) no caben en el tope global de 8 KB.
            body_limit: Some(TURN_BODY_LIMIT),
            handler: post(answer),
        },
        RouteSpec {
            path: "/api/v1/interviews/{id}/finish",
            methods: &["POST"],
            access: Access::Authenticated,
            body_limit: None,
            handler: post(finish),
        },
        RouteSpec {
            path: "/api/v1/interviews/{id}/score",
            methods: &["GET"],
            access: Access::Authenticated,
            body_limit: None,
            handler: get(score),
        },
    ]
}

/// Tope de cuerpo de `POST .../turns`: 4000 caracteres de hasta 4 bytes más el JSON, con margen.
const TURN_BODY_LIMIT: usize = 32 * 1024;
/// Largo máximo de la respuesta, en caracteres Unicode (no bytes), ya recortada.
const MAX_TEXT_CHARS: usize = 4000;

enum ApiError {
    Invalid,
    NotFound,
    Locked,
    Unavailable,
    SessionOpen(Uuid),
    PlanLimit(String),
    BadState,
    Engine,
    /// Otro turno de la misma sesión está en vuelo.
    Busy,
    /// `finish` con menos de 2 respuestas: la sesión sigue abierta.
    TooShort,
    /// Demasiadas respuestas en el último minuto; segundos hasta poder volver.
    TooMany(u64),
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
            Self::TooShort => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "entrevista_muy_corta",
                "Responde al menos dos preguntas antes de terminar la entrevista.",
                None,
            ),
            Self::Engine => (
                StatusCode::SERVICE_UNAVAILABLE,
                "motor_no_disponible",
                "El recruiter no está disponible ahora. Inténtalo en un momento.",
                None,
            ),
            Self::Busy => (
                StatusCode::CONFLICT,
                "turno_en_proceso",
                "Tu respuesta anterior todavía se está procesando. Espera un momento.",
                None,
            ),
            Self::TooMany(segundos) => {
                let cuerpo = json!({
                    "error": "demasiados_intentos",
                    "message": "Estás respondiendo demasiado rápido. Espera un momento.",
                });
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    [(header::RETRY_AFTER, segundos.to_string())],
                    Json(cuerpo),
                )
                    .into_response();
            }
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

/// La oferta del cuerpo (HU-31): ausente o `null` = sin oferta; una cadena que no es un UUID **no es
/// 422**, es una oferta que no existe (PR4: mismo 404 que cualquier oferta no visible); cualquier
/// otro tipo es 422.
enum OfertaPedida {
    Ninguna,
    Id(Uuid),
    Inexistente,
}

/// El cuerpo de `POST /interviews`: dos UUID y, opcional, `job_offer_id`. Cualquier otra cosa es
/// 422, sin repetir nada.
fn leer_cuerpo(cuerpo: &[u8]) -> Result<(Uuid, Uuid, OfertaPedida), ApiError> {
    let json: Value = serde_json::from_slice(cuerpo).map_err(|_| ApiError::Invalid)?;
    let id = |campo: &str| {
        json.get(campo)
            .and_then(Value::as_str)
            .and_then(|t| Uuid::parse_str(t).ok())
            .ok_or(ApiError::Invalid)
    };
    let oferta = match json.get("job_offer_id") {
        None | Some(Value::Null) => OfertaPedida::Ninguna,
        Some(Value::String(t)) => match Uuid::parse_str(t) {
            Ok(u) => OfertaPedida::Id(u),
            Err(_) => OfertaPedida::Inexistente,
        },
        Some(_) => return Err(ApiError::Invalid),
    };
    Ok((id("recruiter_id")?, id("level_id")?, oferta))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    cuerpo: Bytes,
) -> Result<Response, ApiError> {
    // Orden del contrato: cuerpo -> existen (recruiter, nivel y oferta visible, todos 404) ->
    // disponible -> desbloqueado -> sin sesión activa -> cuota (esta última, dentro de la creación).
    let (recruiter_id, level_id, pedida) = leer_cuerpo(&cuerpo)?;
    let applicant = applicant_de(&state, &user).await?;

    let recruiter = store::find_recruiter(&state.db, recruiter_id)
        .await
        .map_err(interno("leer_recruiter"))?
        .ok_or(ApiError::NotFound)?;
    let level = store::find_level(&state.db, applicant, level_id)
        .await
        .map_err(interno("leer_nivel"))?
        .ok_or(ApiError::NotFound)?;
    // Solo una oferta visible para candidatos; el resto, igual que una inexistente.
    let oferta = match pedida {
        OfertaPedida::Ninguna => None,
        OfertaPedida::Inexistente => return Err(ApiError::NotFound),
        OfertaPedida::Id(id) => {
            store::find_visible_offer(&state.db, id)
                .await
                .map_err(interno("leer_oferta"))?
                .ok_or(ApiError::NotFound)?;
            Some(id)
        }
    };
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
    let foco = match oferta {
        Some(id) => store::offer_focus(&state.db, id)
            .await
            .map_err(interno("leer_foco_oferta"))?,
        None => None,
    };
    let primera = match &foco {
        Some(f) => DeterministicEngine.next_turn_for_offer(&config, f, &[]),
        None => DeterministicEngine.next_turn(&config, &[]),
    }
    .map_err(|_| ApiError::Engine)?;

    // Dos creaciones simultáneas pasan la comprobación de arriba: el índice único parcial decide.
    let session_id = match store::create_session_for_offer(
        &state.db,
        &state.pii,
        applicant,
        recruiter.id,
        level.id,
        oferta,
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

// ------------------------------------------------------------------ POST /interviews/{id}/turns

/// El cuerpo de un turno: `{text}` como texto, recortado, ni vacío ni de más de 4000 caracteres.
/// Cualquier otra cosa es 422, sin repetir nada de lo recibido.
fn leer_texto(cuerpo: &[u8]) -> Result<String, ApiError> {
    let json: Value = serde_json::from_slice(cuerpo).map_err(|_| ApiError::Invalid)?;
    let texto = json
        .get("text")
        .and_then(Value::as_str)
        .ok_or(ApiError::Invalid)?
        .trim();
    if texto.is_empty() || texto.chars().count() > MAX_TEXT_CHARS {
        return Err(ApiError::Invalid);
    }
    Ok(texto.to_string())
}

async fn answer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    cuerpo: Bytes,
) -> Result<Response, ApiError> {
    let id = id_de_ruta(&id)?;
    let texto = leer_texto(&cuerpo)?;
    let applicant = applicant_de(&state, &user).await?;
    match store::answer_turn(
        &state.db,
        &state.pii,
        state.engine.as_ref(),
        applicant,
        id,
        &texto,
    )
    .await
    .map_err(interno("responder_turno"))?
    {
        AnswerOutcome::Replied { turn, done } => Ok((
            StatusCode::CREATED,
            Json(json!({ "turn": turn, "done": done })),
        )
            .into_response()),
        AnswerOutcome::NotFound => Err(ApiError::NotFound),
        AnswerOutcome::InvalidState => Err(ApiError::BadState),
        AnswerOutcome::Busy => Err(ApiError::Busy),
        AnswerOutcome::RateLimited { retry_after_secs } => Err(ApiError::TooMany(retry_after_secs)),
        AnswerOutcome::EngineFailed => {
            tracing::error!(donde = "motor", "motor no disponible");
            Err(ApiError::Engine)
        }
    }
}

// ------------------------------------------------------------------ finish y score

async fn finish(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<store::Score>, ApiError> {
    let id = id_de_ruta(&id)?;
    let applicant = applicant_de(&state, &user).await?;
    match store::finish_session(&state.db, &state.pii, state.engine.as_ref(), applicant, id)
        .await
        .map_err(interno("terminar_sesion"))?
    {
        FinishOutcome::Finished(score) => Ok(Json(score)),
        FinishOutcome::NotFound => Err(ApiError::NotFound),
        FinishOutcome::InvalidState => Err(ApiError::BadState),
        FinishOutcome::Busy => Err(ApiError::Busy),
        FinishOutcome::TooShort => Err(ApiError::TooShort),
        FinishOutcome::EngineFailed => {
            tracing::error!(donde = "motor", "motor no disponible");
            Err(ApiError::Engine)
        }
    }
}

async fn score(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<store::Score>, ApiError> {
    let id = id_de_ruta(&id)?;
    let applicant = applicant_de(&state, &user).await?;
    match store::score_of(&state.db, applicant, id)
        .await
        .map_err(interno("leer_score"))?
    {
        ScoreOutcome::Found(score) => Ok(Json(score)),
        ScoreOutcome::NotFound => Err(ApiError::NotFound),
        ScoreOutcome::InvalidState => Err(ApiError::BadState),
    }
}

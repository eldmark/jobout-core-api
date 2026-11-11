//! Catálogo de la entrevista (Fase 3, paso 5): `GET /api/v1/recruiters`, `GET /api/v1/levels` y
//! `GET /api/v1/progress`.
//! `GET /recruiters` añade `ai_replies` (JB-92): si la IA redacta de verdad, para el aviso de privacidad.
//!
//! Solo lectura. Nunca sale de aquí el `system_prompt` de un recruiter ni los criterios internos
//! de la rúbrica: las consultas piden columnas por nombre, no `SELECT *`. Lo que depende de la
//! persona (niveles y progreso) se filtra siempre por el `applicant` del token.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json,
};
use sea_orm::{ConnectionTrait, DbBackend, DbErr, QueryResult, Statement, Value};
use serde::Serialize;
use serde_json::json;

use super::table::{Access, RouteSpec};
use crate::{auth_user::AuthUser, state::AppState};

/// Tope de sesiones que devuelve `GET /progress` (las más recientes).
const PROGRESS_LIMIT: i64 = 200;

pub fn routes() -> Vec<RouteSpec> {
    let ruta = |path, handler| RouteSpec {
        path,
        methods: &["GET"],
        access: Access::Authenticated,
        body_limit: None,
        handler,
    };
    vec![
        ruta("/api/v1/recruiters", get(recruiters)),
        ruta("/api/v1/levels", get(levels)),
        ruta("/api/v1/progress", get(progress)),
    ]
}

struct ApiError;

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "error_interno",
                "message": "Algo salió mal de nuestro lado. Inténtalo de nuevo."
            })),
        )
            .into_response()
    }
}

/// Registra el fallo por categoría, **nunca** con el texto del error de la base.
fn db(donde: &'static str) -> impl Fn(DbErr) -> ApiError {
    move |_| {
        tracing::error!(donde, "error interno");
        ApiError
    }
}

fn stmt(sql: &str, valores: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, valores)
}

#[derive(Serialize)]
struct Items<T> {
    items: Vec<T>,
}

/// `GET /recruiters`: los items de siempre más `ai_replies` (JB-92).
#[derive(Serialize)]
struct Recruiters {
    items: Vec<RecruiterOut>,
    /// `true` solo si un modelo redacta al recruiter (las respuestas viajan a OpenRouter).
    ai_replies: bool,
}

// ------------------------------------------------------------ recruiters

#[derive(Serialize)]
struct RecruiterOut {
    id: String,
    code: String,
    display_name: String,
    role_title: String,
    personality: String,
    language: String,
}

async fn recruiters(
    State(state): State<AppState>,
    _user: AuthUser,
) -> Result<Json<Recruiters>, ApiError> {
    let filas = state
        .db
        .query_all(stmt(
            "SELECT r.id::text, r.code, r.display_name, r.role_title, r.personality, l.code \
             FROM ai_recruiters r JOIN languages l ON l.id = r.language_id \
             WHERE r.active ORDER BY r.sort_order, r.code",
            vec![],
        ))
        .await
        .map_err(db("leer_recruiters"))?;
    let items = filas
        .iter()
        .map(|f| {
            Ok(RecruiterOut {
                id: f.try_get_by_index(0)?,
                code: f.try_get_by_index(1)?,
                display_name: f.try_get_by_index(2)?,
                role_title: f.try_get_by_index(3)?,
                personality: f.try_get_by_index(4)?,
                language: f.try_get_by_index(5)?,
            })
        })
        .collect::<Result<Vec<_>, DbErr>>()
        .map_err(db("leer_fila_recruiter"))?;
    Ok(Json(Recruiters {
        items,
        ai_replies: state.engine.ai_replies(),
    }))
}

// ---------------------------------------------------------------- levels

#[derive(Serialize)]
struct LevelOut {
    id: String,
    code: String,
    name: String,
    position: i32,
    available: bool,
    unlocked: bool,
    min_score_to_unlock: Option<i16>,
    best_score: Option<i16>,
    sessions_completed: i32,
}

fn nivel(f: &QueryResult) -> Result<LevelOut, DbErr> {
    Ok(LevelOut {
        id: f.try_get_by_index(0)?,
        code: f.try_get_by_index(1)?,
        name: f.try_get_by_index(2)?,
        position: f.try_get_by_index(3)?,
        available: f.try_get_by_index(4)?,
        unlocked: f.try_get_by_index(5)?,
        min_score_to_unlock: f.try_get_by_index(6)?,
        best_score: f.try_get_by_index(7)?,
        sessions_completed: f.try_get_by_index(8)?,
    })
}

/// Desbloqueado = disponible **y** (primer nivel, o hay fila de progreso de esta persona).
/// Un nivel que no está disponible queda bloqueado aunque el progreso alcance.
async fn levels(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Items<LevelOut>>, ApiError> {
    let filas = state
        .db
        .query_all(stmt(
            "SELECT l.id::text, l.code, l.display_name, l.sort_order, l.available, \
                    (l.available AND (l.previous_level_id IS NULL OR p.id IS NOT NULL)), \
                    l.min_score_to_unlock, p.best_score, COALESCE(p.sessions_completed, 0) \
             FROM interview_levels l \
             LEFT JOIN applicant_level_progress p \
               ON p.interview_level_id = l.id \
              AND p.applicant_id = (SELECT id FROM applicants WHERE user_id = $1::uuid) \
             WHERE l.active ORDER BY l.sort_order",
            vec![user.id.clone().into()],
        ))
        .await
        .map_err(db("leer_niveles"))?;
    let items = filas
        .iter()
        .map(nivel)
        .collect::<Result<Vec<_>, DbErr>>()
        .map_err(db("leer_fila_nivel"))?;
    Ok(Json(Items { items }))
}

// -------------------------------------------------------------- progress

#[derive(Serialize)]
struct ProgressOut {
    session_id: String,
    completed_at: String,
    level: String,
    recruiter: String,
    score: i16,
}

/// Las 200 sesiones completadas más recientes **de esta persona**, de la más antigua a la más
/// nueva.
async fn progress(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Items<ProgressOut>>, ApiError> {
    let filas = state
        .db
        .query_all(stmt(
            "SELECT * FROM ( \
               SELECT s.id::text AS session_id, \
                      to_char(s.completed_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS completed_at, \
                      l.display_name AS level, r.display_name AS recruiter, sc.score AS score, \
                      s.completed_at AS orden \
               FROM interview_sessions s \
               JOIN interview_scores sc ON sc.interview_session_id = s.id \
               JOIN interview_levels l ON l.id = s.interview_level_id \
               JOIN ai_recruiters r ON r.id = s.ai_recruiter_id \
               WHERE s.applicant_id = (SELECT id FROM applicants WHERE user_id = $1::uuid) \
                 AND s.status = 'completed' AND s.completed_at IS NOT NULL \
               ORDER BY s.completed_at DESC, s.id DESC LIMIT $2 \
             ) recientes ORDER BY orden ASC, session_id ASC",
            vec![user.id.clone().into(), PROGRESS_LIMIT.into()],
        ))
        .await
        .map_err(db("leer_progreso"))?;
    let items = filas
        .iter()
        .map(|f| {
            Ok(ProgressOut {
                session_id: f.try_get_by_index(0)?,
                completed_at: f.try_get_by_index(1)?,
                level: f.try_get_by_index(2)?,
                recruiter: f.try_get_by_index(3)?,
                score: f.try_get_by_index(4)?,
            })
        })
        .collect::<Result<Vec<_>, DbErr>>()
        .map_err(db("leer_fila_progreso"))?;
    Ok(Json(Items { items }))
}

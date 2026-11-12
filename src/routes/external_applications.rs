//! `/api/v1/me/external-applications`: registro privado de aplicaciones hechas fuera de la
//! plataforma (HU-32, decisión 6). Nadie más lo ve; no pasa por la máquina de estados ni por el
//! consentimiento. `notes` va cifrada. Todo filtra por dueño: ajeno, inexistente o con id mal
//! formado → 404.

use axum::{
    body::Bytes,
    extract::{rejection::BytesRejection, Path, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait, Value};
use serde_json::{json, Value as JsonValue};

use super::applications::{cuerpo, db, ApiError};
use super::company_support::{codificar_cursor, parse_consulta, Pagina};
use super::table::{Access, RouteSpec};
use crate::{
    applications::{
        external::{parse_external, ExternalError, ExternalInput, MAX_RECORDS, STATUSES},
        store,
    },
    auth_user::AuthUser,
    offers::{
        input::{fecha, uuid_canonico},
        store as offers_store,
    },
    state::AppState,
};

pub fn routes() -> Vec<RouteSpec> {
    vec![
        RouteSpec {
            path: "/api/v1/me/external-applications",
            methods: &["POST", "GET"],
            access: Access::Authenticated,
            body_limit: None,
            handler: post(create).get(list),
        },
        RouteSpec {
            path: "/api/v1/me/external-applications/{id}",
            methods: &["GET", "PUT", "DELETE"],
            access: Access::Authenticated,
            body_limit: None,
            handler: get(read).put(replace).delete(remove),
        },
    ]
}

fn stmt(sql: &str, v: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, v)
}

impl From<ExternalError> for ApiError {
    fn from(e: ExternalError) -> Self {
        match e {
            ExternalError::UnknownField => Self::UnknownField,
            ExternalError::Invalid(m) => Self::Invalid(m),
        }
    }
}

async fn entrada(
    s: &AppState,
    b: Result<Bytes, BytesRejection>,
) -> Result<ExternalInput, ApiError> {
    let bytes = cuerpo(b)?;
    let hoy = offers_store::today(&s.db).await.map_err(db("hoy"))?;
    let hoy = fecha(&hoy).ok_or(ApiError::Internal)?;
    Ok(parse_external(&bytes, hoy)?)
}

const COLS: &str =
    "e.id::text, e.company_name, e.position, e.applied_on::text, e.status, e.notes, \
    to_char(e.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'), \
    ((e.applied_on - date '1970-01-01')::bigint * 86400000000)";

fn fila_json(s: &AppState, f: &sea_orm::QueryResult) -> Result<(JsonValue, i64), ApiError> {
    let i = |n| db("leer_externa")(sea_orm::DbErr::Custom(format!("col {n}")));
    let notas: Option<Vec<u8>> = f.try_get_by_index(5).map_err(|_| i(5))?;
    let notas = match notas {
        Some(b) => Some(s.pii.decrypt(&b).map_err(|_| {
            tracing::error!(donde = "descifrar_notas", "error interno");
            ApiError::Internal
        })?),
        None => None,
    };
    let g = |n: usize| f.try_get_by_index::<String>(n).map_err(|_| i(n));
    Ok((
        json!({
            "id": g(0)?, "company_name": g(1)?, "position": g(2)?, "applied_on": g(3)?,
            "status": g(4)?, "notes": notas, "created_at": g(6)?,
        }),
        f.try_get_by_index::<i64>(7).map_err(|_| i(7))?,
    ))
}

async fn por_id(s: &AppState, user: &str, id: &str) -> Result<JsonValue, ApiError> {
    let f =
        s.db.query_one(stmt(
            &format!(
                "SELECT {COLS} FROM external_applications e \
                 JOIN applicants a ON a.id = e.applicant_id AND a.user_id = $1::uuid \
                 WHERE e.id = $2::uuid"
            ),
            vec![user.to_string().into(), id.to_string().into()],
        ))
        .await
        .map_err(db("leer_externa"))?
        .ok_or(ApiError::NotFound)?;
    Ok(fila_json(s, &f)?.0)
}

fn id_de_path(id: &str) -> Result<String, ApiError> {
    uuid_canonico(id).ok_or(ApiError::NotFound)
}

fn cifrar(s: &AppState, e: &ExternalInput) -> Result<Option<Vec<u8>>, ApiError> {
    e.notes
        .as_deref()
        .map(|n| {
            s.pii.encrypt(n).map_err(|_| {
                tracing::error!(donde = "cifrar_notas", "error interno");
                ApiError::Internal
            })
        })
        .transpose()
}

async fn create(
    State(s): State<AppState>,
    user: AuthUser,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let e = entrada(&s, body).await?;
    let notas = cifrar(&s, &e)?;
    let txn = s.db.begin().await.map_err(db("externa_begin"))?;
    store::lock_applicant(&txn, &user.id)
        .await
        .map_err(db("externa_candado"))?;
    let applicant = store::applicant_of(&txn, &user.id)
        .await
        .map_err(db("externa_persona"))?
        .ok_or(ApiError::NotFound)?;
    let n: i64 = txn
        .query_one(stmt(
            "SELECT count(*) FROM external_applications WHERE applicant_id = $1::uuid",
            vec![applicant.clone().into()],
        ))
        .await
        .map_err(db("externa_contar"))?
        .ok_or(ApiError::Internal)?
        .try_get_by_index(0)
        .map_err(db("externa_contar_n"))?;
    if n >= MAX_RECORDS {
        return Err(ApiError::Conflict(
            "limite_de_registros",
            "Llegaste al máximo de registros externos.",
        ));
    }
    let id = migration::new_id().to_string();
    txn.execute(stmt(
        "INSERT INTO external_applications (id, applicant_id, company_name, position, applied_on, status, notes) \
         VALUES ($1::uuid, $2::uuid, $3, $4, $5::date, $6, $7)",
        vec![
            id.clone().into(), applicant.into(), e.company_name.into(), e.position.into(),
            e.applied_on.into(), e.status.into(), notas.into(),
        ],
    ))
    .await
    .map_err(db("externa_insertar"))?;
    txn.commit().await.map_err(db("externa_commit"))?;
    Ok((StatusCode::CREATED, Json(por_id(&s, &user.id, &id).await?)).into_response())
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
        Some(e) if STATUSES.contains(&e.as_str()) => Some(e.clone()),
        Some(_) => return Err(ApiError::Invalid("El estado no es válido.")),
    };
    let (micros, cid) = match pagina.cursor {
        Some((m, i)) => (Some(m), Some(i)),
        None => (None, None),
    };
    let filas =
        s.db.query_all(stmt(
            &format!(
                "SELECT {COLS} FROM external_applications e \
                 JOIN applicants a ON a.id = e.applicant_id AND a.user_id = $1::uuid \
                 WHERE ($2::text IS NULL OR e.status = $2) \
                   AND ($3::bigint IS NULL OR (e.applied_on, e.id) < \
                        ((date '1970-01-01' + ($3::bigint / 86400000000)::int), $4::uuid)) \
                 ORDER BY e.applied_on DESC, e.id DESC LIMIT $5"
            ),
            vec![
                user.id.clone().into(),
                estado.into(),
                micros.into(),
                cid.into(),
                (pagina.limite + 1).into(),
            ],
        ))
        .await
        .map_err(db("listar_externas"))?;
    let hay_mas = filas.len() as i64 > pagina.limite;
    let mut items = Vec::new();
    let mut ultimo = None;
    for f in filas.iter().take(pagina.limite as usize) {
        let (v, m) = fila_json(&s, f)?;
        ultimo = Some((m, v["id"].as_str().unwrap_or_default().to_string()));
        items.push(v);
    }
    let siguiente = if hay_mas {
        ultimo.map(|(m, id)| codificar_cursor(m, &id))
    } else {
        None
    };
    Ok(Json(json!({"items": items, "next_cursor": siguiente})).into_response())
}

async fn read(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = id_de_path(&id)?;
    Ok(Json(por_id(&s, &user.id, &id).await?).into_response())
}

async fn replace(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let id = id_de_path(&id)?;
    let e = entrada(&s, body).await?;
    let notas = cifrar(&s, &e)?;
    let n = s
        .db
        .execute(stmt(
            "UPDATE external_applications SET company_name = $3, position = $4, applied_on = $5::date, \
                status = $6, notes = $7, updated_at = now() \
             WHERE id = $1::uuid AND applicant_id = (SELECT id FROM applicants WHERE user_id = $2::uuid)",
            vec![
                id.into(), user.id.into(), e.company_name.into(), e.position.into(),
                e.applied_on.into(), e.status.into(), notas.into(),
            ],
        ))
        .await
        .map_err(db("reemplazar_externa"))?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn remove(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = id_de_path(&id)?;
    let n = s
        .db
        .execute(stmt(
            "DELETE FROM external_applications \
             WHERE id = $1::uuid AND applicant_id = (SELECT id FROM applicants WHERE user_id = $2::uuid)",
            vec![id.into(), user.id.into()],
        ))
        .await
        .map_err(db("borrar_externa"))?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

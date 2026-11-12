//! Panel del reclutador (Fase 5a, paso 7, HU-52 a HU-54). Contrato: `api.md`, «Panel del
//! reclutador».
//!
//! Es la ruta por la que los datos de una persona llegan a una empresa, así que:
//!
//! - La empresa sale de la membresía de quien llama (`company_scope::company_of`), nunca del path.
//!   Una oferta o aplicación de otra empresa, una inexistente y un id mal formado dan el **mismo**
//!   404 (SX2, SX6).
//! - La lista solo trae el nombre visible; el detalle, solo lo que la persona marcó
//!   (`applications::shared_view`). Nada de la persona ni de la nota llega a un log ni a un error.
//! - Cada respuesta con datos de una persona deja su fila en `access_audit` **en la misma
//!   transacción**: si la fila no se escribe, la petición falla y el dato no sale (PA8).
//! - Orden de validación: 401 → 403 `sin_permiso` (tabla de rutas) → 413 → 422 del cuerpo o la
//!   consulta → 404 → 403 de la empresa → 409 → 429.

use axum::{
    body::Bytes,
    extract::{rejection::BytesRejection, Path, RawQuery, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json,
};
use sea_orm::{DbErr, TransactionTrait};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::company_support::{
    self, auditar_recurso, candado, codificar_cursor, parse_cerrado, parse_consulta,
    texto_opcional, IpCliente, Pagina,
};
use super::table::{Access, RouteSpec};
use crate::{
    applications::{
        move_status,
        panel::{self, Order, OrderError, PanelRow, Query},
        shared_view, Availability, MoveError, Scope, Status,
    },
    auth_user::AuthUser,
    company_gate::{self, Gate},
    company_scope::company_of,
    matching::store as matching_store,
    offers::input::uuid_canonico,
    state::AppState,
};

/// Detalles que un reclutador puede leer por hora (SX5, anti-raspado).
pub const VIEWS_PER_HOUR: i64 = 120;
/// Largo máximo de la nota interna de un cambio de estado, en caracteres.
pub const NOTE_MAX_CHARS: usize = 500;

pub fn routes() -> Vec<RouteSpec> {
    vec![
        RouteSpec {
            path: "/api/v1/company/offers/{id}/applications",
            methods: &["GET"],
            access: Access::Permission("application.read"),
            body_limit: None,
            handler: get(list),
        },
        RouteSpec {
            path: "/api/v1/company/applications/{id}",
            methods: &["GET"],
            access: Access::Permission("application.read"),
            body_limit: None,
            handler: get(read),
        },
        RouteSpec {
            path: "/api/v1/company/applications/{id}/status",
            methods: &["POST"],
            access: Access::Permission("application.review"),
            body_limit: None,
            handler: post(change_status),
        },
        RouteSpec {
            path: "/api/v1/company/applications/{id}/favorite",
            methods: &["PUT", "DELETE"],
            access: Access::Permission("favorite.manage"),
            body_limit: None,
            handler: put(mark_favorite).delete(unmark_favorite),
        },
    ]
}

// ------------------------------------------------------------------ errores

enum ApiError {
    /// Ajeno, inexistente, id mal formado o persona sin empresa: siempre el mismo cuerpo.
    NotFound,
    TooLarge,
    Invalid(&'static str),
    UnknownField,
    UnknownParam,
    /// 403 de la empresa: sin verificar o con la membresía vencida.
    Company(Gate),
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
            Self::Company(Gate::NotVerified) => error(
                StatusCode::FORBIDDEN,
                "empresa_no_verificada",
                "Tu empresa todavía no está verificada.",
            ),
            Self::Company(Gate::MembershipExpired) => error(
                StatusCode::FORBIDDEN,
                "membresia_vencida",
                "La membresía de tu empresa no está vigente.",
            ),
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
                    "message": "Abriste demasiadas aplicaciones en la última hora. Inténtalo más tarde.",
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

impl From<company_support::ApiError> for ApiError {
    fn from(e: company_support::ApiError) -> Self {
        use company_support::ApiError as E;
        match e {
            E::NotFound => Self::NotFound,
            E::ParamNotAllowed => Self::UnknownParam,
            E::FieldNotAllowed => Self::UnknownField,
            E::Invalid(m) => Self::Invalid(m),
            _ => Self::Internal,
        }
    }
}

/// Registra el fallo por categoría, **nunca** con el texto de la base (podría citar datos).
fn db(donde: &'static str) -> impl Fn(DbErr) -> ApiError {
    move |_| {
        tracing::error!(donde, "error interno");
        ApiError::Internal
    }
}

fn cuerpo(b: Result<Bytes, BytesRejection>) -> Result<Bytes, ApiError> {
    b.map_err(|e| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::TooLarge
        } else {
            ApiError::Invalid("El cuerpo no tiene el formato esperado.")
        }
    })
}

// ------------------------------------------------------------------ entrada

/// Lo que se pide a la lista: filtros y orden **cerrados** (PA9).
#[derive(Debug, PartialEq, Eq)]
struct ListParams {
    status: Option<Status>,
    favorite: Option<bool>,
    order: Order,
    limit: i64,
    cursor: Option<(i64, String)>,
}

const LIST_PARAMS: [&str; 5] = ["limit", "cursor", "status", "favorite", "sort"];

fn parse_list(raw: Option<&str>) -> Result<ListParams, ApiError> {
    let q = parse_consulta(raw, &LIST_PARAMS)?;
    let order = Order::parse(q.get("sort").map(String::as_str)).map_err(|e| match e {
        OrderError::Invalid => ApiError::Invalid("El orden pedido no existe."),
        // Nombre, edad, puntaje…: el panel no ordena por nada de la persona.
        OrderError::NotAllowed => ApiError::UnknownParam,
    })?;
    let status = match q.get("status") {
        None => None,
        Some(s) => Some(Status::parse(s).ok_or(ApiError::Invalid("El estado no es válido."))?),
    };
    let favorite = match q.get("favorite") {
        None => None,
        Some(f) => Some(
            panel::parse_favorite(f)
                .ok_or(ApiError::Invalid("El filtro de favoritos no es válido."))?,
        ),
    };
    let pagina = Pagina::de(&q)?;
    Ok(ListParams {
        status,
        favorite,
        order,
        limit: pagina.limite,
        cursor: pagina.cursor,
    })
}

#[derive(Deserialize)]
struct StatusBody {
    to: String,
    note: Option<String>,
}

/// `{to, note?}`: `to` es un estado al que el reclutador puede mover (nunca `applied` ni
/// `withdrawn`) y la nota, de hasta 500 caracteres; vacía equivale a ausente.
fn parse_status(bytes: &Bytes) -> Result<(Status, Option<String>), ApiError> {
    let b: StatusBody = parse_cerrado(bytes, &["to", "note"])?;
    let to = Status::parse(&b.to)
        .filter(|s| !matches!(s, Status::Applied | Status::Withdrawn))
        .ok_or(ApiError::Invalid("El estado pedido no es válido."))?;
    let note = texto_opcional(&b.note, NOTE_MAX_CHARS, "La nota es demasiado larga.")?;
    Ok((to, note))
}

// ------------------------------------------------------------------ salida

/// `PanelRow`: lo mínimo. Fuera de `available`, `applicant` es `null` y no hay `shared_fields`.
fn row_json(r: &PanelRow) -> Value {
    let mut v = json!({
        "id": r.id,
        "status": r.status.as_str(),
        "applied_at": r.applied_at,
        "status_changed_at": r.status_changed_at,
        "is_favorite": r.is_favorite,
        "availability": r.availability.as_str(),
        "applicant": Value::Null,
    });
    if let (Availability::Available, Some(nombre)) = (r.availability, &r.display_name) {
        v["applicant"] = json!({"display_name": nombre});
        v["shared_fields"] = json!(r.shared_fields);
    }
    v
}

/// `PanelApplication`: la fila, la oferta, lo compartido (solo lo marcado) y el historial.
async fn detail_json<C: sea_orm::ConnectionTrait>(
    s: &AppState,
    c: &C,
    r: &PanelRow,
) -> Result<Value, ApiError> {
    let mut v = row_json(r);
    v["applicant"] = Value::Null;
    if let Some(obj) = v.as_object_mut() {
        obj.remove("shared_fields");
    }
    if r.availability == Availability::Available {
        if let Some(compartido) = shared_view(c, &s.pii, &r.id, r.now_micros)
            .await
            .map_err(db("panel_compartido"))?
        {
            // El puntaje solo si la persona lo marcó, y calculado únicamente con `compartido`
            // (lo consentido), nunca con el perfil entero.
            let mut compartido = compartido;
            if r.shared_fields.iter().any(|f| f == "match_score") {
                if let Some(m) = matching_store::for_shared_view(c, &r.offer_id, &compartido)
                    .await
                    .map_err(db("panel_matching"))?
                {
                    compartido["match_score"] = m;
                }
            }
            v["applicant"] = compartido;
            v["shared_fields"] = json!(r.shared_fields);
        }
    }
    v["offer"] = json!({"id": r.offer_id, "title": r.offer_title});
    let pasos = panel::history(c, &s.pii, &r.id)
        .await
        .map_err(db("panel_historial"))?;
    // Sin quién decidió (BO3); la nota es interna de la empresa.
    v["history"] = json!(pasos
        .iter()
        .map(|p| json!({"from": p.from, "to": p.to, "at": p.at, "note": p.note}))
        .collect::<Vec<_>>());
    Ok(v)
}

// ------------------------------------------------------------------ comunes

/// La empresa de quien llama (404 si no tiene) y, si se da, la aplicación de esa empresa.
async fn empresa(s: &AppState, user: &AuthUser) -> Result<String, ApiError> {
    Ok(company_of(&s.db, &user.id)
        .await
        .map_err(db("panel_empresa"))?
        .ok_or(ApiError::NotFound)?
        .company_id)
}

async fn aplicacion(s: &AppState, company: &str, id: &str) -> Result<PanelRow, ApiError> {
    panel::one(&s.db, company, id)
        .await
        .map_err(db("panel_aplicacion"))?
        .ok_or(ApiError::NotFound)
}

/// 403 si la empresa no está verificada o su membresía no está vigente, en cada petición.
async fn vigente(s: &AppState, company: &str) -> Result<(), ApiError> {
    let id = Uuid::parse_str(company).map_err(|_| ApiError::Internal)?;
    company_gate::check(&s.db, id)
        .await
        .map_err(db("panel_vigencia"))?
        .map_err(ApiError::Company)
}

// ------------------------------------------------------------------ rutas

/// `GET /company/offers/{id}/applications`.
async fn list(
    State(s): State<AppState>,
    user: AuthUser,
    ip: IpCliente,
    Path(offer): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let p = parse_list(raw.as_deref())?;
    let offer = uuid_canonico(&offer).ok_or(ApiError::NotFound)?;
    let company = empresa(&s, &user).await?;
    if !panel::offer_of_company(&s.db, &company, &offer)
        .await
        .map_err(db("panel_oferta"))?
    {
        return Err(ApiError::NotFound);
    }
    vigente(&s, &company).await?;

    let txn = s.db.begin().await.map_err(db("panel_lista_begin"))?;
    let mut filas = panel::list(
        &txn,
        &Query {
            company: &company,
            offer: Some(&offer),
            id: None,
            status: p.status,
            favorite: p.favorite,
            cursor: p.cursor,
            order: p.order,
            limit: p.limit + 1,
        },
    )
    .await
    .map_err(db("panel_lista"))?;
    auditar_recurso(
        &txn,
        &s.pii,
        &user.id,
        "application.list",
        "job_offer",
        Some(&offer),
        &ip,
    )
    .await?;
    txn.commit().await.map_err(db("panel_lista_commit"))?;

    let hay_mas = filas.len() as i64 > p.limit;
    filas.truncate(p.limit as usize);
    let siguiente = if hay_mas {
        filas
            .last()
            .map(|f| codificar_cursor(f.applied_micros, &f.id))
    } else {
        None
    };
    Ok(Json(json!({
        "items": filas.iter().map(row_json).collect::<Vec<_>>(),
        "next_cursor": siguiente,
    }))
    .into_response())
}

/// `GET /company/applications/{id}`: 404 → 403 → 429 → dato y su auditoría, juntos.
async fn read(
    State(s): State<AppState>,
    user: AuthUser,
    ip: IpCliente,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = uuid_canonico(&id).ok_or(ApiError::NotFound)?;
    let company = empresa(&s, &user).await?;
    aplicacion(&s, &company, &id).await?;
    vigente(&s, &company).await?;

    let txn = s.db.begin().await.map_err(db("panel_detalle_begin"))?;
    // Las lecturas de un mismo reclutador se cuentan de a una: el tope no se pasa en paralelo.
    candado(&txn, &format!("panel:{}", user.id), "panel_candado").await?;
    let (n, espera) = panel::views_last_hour(&txn, &user.id)
        .await
        .map_err(db("panel_cupo"))?;
    if n >= VIEWS_PER_HOUR {
        return Err(ApiError::TooMany(espera));
    }
    let fila = panel::one(&txn, &company, &id)
        .await
        .map_err(db("panel_detalle"))?
        .ok_or(ApiError::NotFound)?;
    let v = detail_json(&s, &txn, &fila).await?;
    auditar_recurso(
        &txn,
        &s.pii,
        &user.id,
        "application.view",
        "job_application",
        Some(&id),
        &ip,
    )
    .await?;
    txn.commit().await.map_err(db("panel_detalle_commit"))?;
    Ok(Json(v).into_response())
}

/// `POST /company/applications/{id}/status`: cuerpo → 404 → 403 → 409; el cambio, el evento y
/// la auditoría en una transacción.
async fn change_status(
    State(s): State<AppState>,
    user: AuthUser,
    ip: IpCliente,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let (to, note) = parse_status(&cuerpo(body)?)?;
    let id = uuid_canonico(&id).ok_or(ApiError::NotFound)?;
    let company = empresa(&s, &user).await?;
    let fila = aplicacion(&s, &company, &id).await?;
    vigente(&s, &company).await?;
    // Sin datos que ver no hay nada que decidir: una entrada anónima, retirada o con la
    // retención vencida no se mueve.
    if fila.availability != Availability::Available {
        return Err(ApiError::BadState(fila.status));
    }

    let txn = s.db.begin().await.map_err(db("panel_estado_begin"))?;
    match move_status(
        &txn,
        &s.pii,
        Scope::Company {
            company_id: &company,
            user_id: &user.id,
        },
        &id,
        to,
        note.as_deref(),
    )
    .await
    {
        Ok(_) => {}
        Err(MoveError::NotFound) => return Err(ApiError::NotFound),
        Err(MoveError::Invalid(actual)) => return Err(ApiError::BadState(actual)),
        Err(MoveError::Crypto) | Err(MoveError::Db(_)) => {
            tracing::error!(donde = "panel_mover", "error interno");
            return Err(ApiError::Internal);
        }
    }
    auditar_recurso(
        &txn,
        &s.pii,
        &user.id,
        "application.review",
        "job_application",
        Some(&id),
        &ip,
    )
    .await?;
    let fila = panel::one(&txn, &company, &id)
        .await
        .map_err(db("panel_estado_releer"))?
        .ok_or(ApiError::Internal)?;
    let v = detail_json(&s, &txn, &fila).await?;
    txn.commit().await.map_err(db("panel_estado_commit"))?;
    Ok(Json(v).into_response())
}

/// `PUT /company/applications/{id}/favorite`: idempotente; 409 si la aplicación no está
/// disponible.
async fn mark_favorite(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = uuid_canonico(&id).ok_or(ApiError::NotFound)?;
    let company = empresa(&s, &user).await?;
    let fila = aplicacion(&s, &company, &id).await?;
    vigente(&s, &company).await?;
    let (Availability::Available, Some(persona)) = (fila.availability, &fila.applicant_id) else {
        return Err(ApiError::BadState(fila.status));
    };
    panel::add_favorite(&s.db, &company, persona)
        .await
        .map_err(db("panel_favorito"))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `DELETE /company/applications/{id}/favorite`: idempotente.
async fn unmark_favorite(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = uuid_canonico(&id).ok_or(ApiError::NotFound)?;
    let company = empresa(&s, &user).await?;
    let fila = aplicacion(&s, &company, &id).await?;
    vigente(&s, &company).await?;
    if let Some(persona) = &fila.applicant_id {
        panel::remove_favorite(&s.db, &company, persona)
            .await
            .map_err(db("panel_quitar_favorito"))?;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lista(q: &str) -> Result<ListParams, &'static str> {
        parse_list(Some(q)).map_err(|e| match e {
            ApiError::UnknownParam => "parametro_no_permitido",
            ApiError::Invalid(_) => "datos_invalidos",
            _ => "otro",
        })
    }

    #[test]
    fn la_consulta_vacia_es_reciente_de_a_veinte() {
        assert_eq!(
            lista("").unwrap(),
            ListParams {
                status: None,
                favorite: None,
                order: Order::Recent,
                limit: 20,
                cursor: None,
            }
        );
    }

    #[test]
    fn filtros_y_orden_permitidos() {
        let p = lista("status=shortlisted&favorite=true&sort=oldest&limit=5").unwrap();
        assert_eq!(p.status, Some(Status::Shortlisted));
        assert_eq!(p.favorite, Some(true));
        assert_eq!(p.order, Order::Oldest);
        assert_eq!(p.limit, 5);
    }

    #[test]
    fn nada_de_la_persona_se_puede_pedir() {
        for q in [
            "age=1",
            "gender=f",
            "name=a",
            "sort=name",
            "q=x",
            "skill_id=1",
        ] {
            assert_eq!(lista(q).unwrap_err(), "parametro_no_permitido", "{q}");
        }
        for q in ["status=foo", "favorite=1", "sort=", "limit=0", "limit=51"] {
            assert_eq!(lista(q).unwrap_err(), "datos_invalidos", "{q}");
        }
    }

    fn estado(cuerpo: &str) -> Result<(Status, Option<String>), &'static str> {
        parse_status(&Bytes::from(cuerpo.to_string())).map_err(|e| match e {
            ApiError::UnknownField => "campo_no_permitido",
            ApiError::Invalid(_) => "datos_invalidos",
            _ => "otro",
        })
    }

    #[test]
    fn el_reclutador_solo_pide_sus_estados() {
        assert_eq!(
            estado(r#"{"to":"under_review"}"#).unwrap(),
            (Status::UnderReview, None)
        );
        assert_eq!(
            estado(r#"{"to":"hired","note":"  bien  "}"#).unwrap(),
            (Status::Hired, Some("bien".into()))
        );
        assert_eq!(estado(r#"{"to":"rejected","note":""}"#).unwrap().1, None);
        for malo in [
            r#"{"to":"applied"}"#,
            r#"{"to":"withdrawn"}"#,
            r#"{"to":"closed"}"#,
            r#"{}"#,
            r#"{"to":1}"#,
            r#"[]"#,
            "no es json",
        ] {
            assert_eq!(estado(malo).unwrap_err(), "datos_invalidos", "{malo}");
        }
        assert_eq!(
            estado(r#"{"to":"hired","reason":"x"}"#).unwrap_err(),
            "campo_no_permitido"
        );
    }

    #[test]
    fn la_nota_tiene_tope_de_500_caracteres() {
        let justa = format!(r#"{{"to":"hired","note":"{}"}}"#, "ñ".repeat(500));
        assert!(estado(&justa).is_ok());
        let larga = format!(r#"{{"to":"hired","note":"{}"}}"#, "ñ".repeat(501));
        assert_eq!(estado(&larga).unwrap_err(), "datos_invalidos");
    }
}

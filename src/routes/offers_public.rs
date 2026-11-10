//! `GET /api/v1/offers` y `GET /api/v1/offers/{id}`: las ofertas que ve un candidato (Fase 5a,
//! paso 5). Contrato: `api.md`, «Ofertas para candidatos» y «No discriminación».
//!
//! - Visible = `published`, con fecha de contratación no vencida, de una empresa verificada, activa
//!   y con membresía vigente (`company_gate::VISIBLE_COMPANY_SQL`, se mira en cada petición).
//!   Todo lo demás, incluido un id mal formado, es el mismo 404 `no_encontrado`.
//! - La consulta es **cerrada**: un parámetro fuera del conjunto es 422 `parametro_no_permitido`
//!   antes de tocar la base. No hay filtro, orden ni búsqueda por empresa ni por texto.
//! - Una oferta confidencial no lee ni una columna de la empresa: `company` es `null`.
//! - Nunca sale `created_by`, el estado, los contadores ni nada del reclutador. Cada petición hace
//!   una sola consulta de datos (sin N+1): el JSON se arma en SQL.
//! - Mirar no escribe nada.

use axum::{
    body::Bytes,
    extract::{rejection::BytesRejection, Path, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sea_orm::{ConnectionTrait, DbErr};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    company_support::{db, parse_consulta, stmt, ApiError, Pagina},
    table::{Access, RouteSpec},
};
use crate::{company_gate::VISIBLE_COMPANY_SQL, offers::input, state::AppState};

/// Los únicos parámetros de `GET /offers`.
const PERMITIDOS: [&str; 11] = [
    "limit",
    "cursor",
    "sort",
    "profession_id",
    "state_id",
    "city_id",
    "contract_type_id",
    "salary_range_id",
    "working_shift_id",
    "employment_sector_id",
    "job_hierarchy_id",
];

/// Columna de la oferta de cada filtro, en el orden de `PERMITIDOS[3..]`.
const FILTROS: [&str; 8] = [
    "profession_id",
    "state_id",
    "city_id",
    "contract_type_id",
    "salary_range_id",
    "working_shift_id",
    "employment_sector_id",
    "job_hierarchy_id",
];

pub fn routes() -> Vec<RouteSpec> {
    vec![
        RouteSpec {
            path: "/api/v1/offers",
            methods: &["GET"],
            access: Access::Authenticated,
            body_limit: None,
            handler: get(list),
        },
        RouteSpec {
            path: "/api/v1/offers/{id}",
            methods: &["GET"],
            access: Access::Authenticated,
            body_limit: None,
            handler: get(read),
        },
    ]
}

/// Fallos de estas rutas: los del grupo más el 413 propio (en JSON) del cuerpo de un `GET`.
enum Fallo {
    Api(ApiError),
    TooLarge,
}

impl From<ApiError> for Fallo {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}

impl IntoResponse for Fallo {
    fn into_response(self) -> Response {
        match self {
            Self::Api(e) => e.into_response(),
            Self::TooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({
                    "error": "cuerpo_demasiado_grande",
                    "message": "El cuerpo de la petición es demasiado grande.",
                })),
            )
                .into_response(),
        }
    }
}

/// El cuerpo de un `GET` se lee para aplicar el tope; va antes que cualquier 422 de la consulta.
fn cuerpo(b: Result<Bytes, BytesRejection>) -> Result<(), Fallo> {
    match b {
        Ok(_) => Ok(()),
        Err(e) if e.status() == StatusCode::PAYLOAD_TOO_LARGE => Err(Fallo::TooLarge),
        Err(_) => Err(ApiError::Invalid("El cuerpo no tiene el formato esperado.").into()),
    }
}

// ------------------------------------------------------------------ SQL

/// `{id, code, name}` de un catálogo, o `null` si la oferta no lo trae.
fn catalogo(tabla: &str, columna: &str) -> String {
    format!(
        "(SELECT jsonb_build_object('id', x.id, 'code', x.code, 'name', x.name) \
          FROM {tabla} x WHERE x.id = o.{columna})"
    )
}

/// El resumen que comparten el listado y el detalle. Una oferta confidencial **no lee** la empresa.
fn resumen_sql() -> String {
    format!(
        "'id', o.id, 'title', o.name, 'confidential', o.is_confidential, \
         'company', CASE WHEN o.is_confidential THEN NULL \
                    ELSE jsonb_build_object('id', c.id, 'name', c.name, 'website', c.website) END, \
         'profession', {}, 'employment_sector', {}, 'state', {}, 'city', {}, \
         'contract_type', {}, 'salary_range', {}, 'working_shift', {}, \
         'vacancies', o.vacancies, 'publication_date', o.publication_date::text",
        catalogo("professions", "profession_id"),
        catalogo("employment_sectors", "employment_sector_id"),
        catalogo("states", "state_id"),
        catalogo("cities", "city_id"),
        catalogo("contract_types", "contract_type_id"),
        catalogo("salary_ranges", "salary_range_id"),
        catalogo("working_shifts", "working_shift_id"),
    )
}

/// Lo que suma el detalle: los requisitos y los textos de la oferta.
fn detalle_sql() -> String {
    format!(
        "'description', o.description, 'schedule', o.schedule, 'hiring_date', o.hiring_date::text, \
         'required_experience', o.required_experience, \
         'required_availability_to_travel', o.required_availability_to_travel, \
         'required_driver_license', o.required_driver_license, \
         'main_language', {}, 'job_hierarchy', {}, 'minimum_academic_level', {}, \
         'skills', COALESCE((SELECT jsonb_agg(jsonb_build_object( \
                'skill', jsonb_build_object('id', s.id, 'code', s.code, 'name', s.name), \
                'is_required', r.is_required, 'min_level', r.min_level::text) \
                ORDER BY r.is_required DESC, s.code, s.id) \
              FROM job_offer_skills r JOIN skills s ON s.id = r.skill_id \
              WHERE r.job_offer_id = o.id), '[]'::jsonb), \
         'languages', COALESCE((SELECT jsonb_agg(jsonb_build_object( \
                'language', jsonb_build_object('id', l.id, 'code', l.code, 'name', l.name), \
                'min_proficiency', r.min_proficiency::text) ORDER BY l.code, l.id) \
              FROM job_offer_languages r JOIN languages l ON l.id = r.language_id \
              WHERE r.job_offer_id = o.id), '[]'::jsonb), \
         'driver_licenses', COALESCE((SELECT jsonb_agg(jsonb_build_object( \
                'id', d.id, 'code', d.code, 'name', d.name) ORDER BY d.code, d.id) \
              FROM job_offer_driver_licenses r JOIN driver_licenses d ON d.id = r.driver_license_id \
              WHERE r.job_offer_id = o.id), '[]'::jsonb)",
        catalogo("languages", "main_language_id"),
        catalogo("job_hierarchies", "job_hierarchy_id"),
        catalogo("academic_levels", "minimum_academic_level_id"),
    )
}

/// `FROM`/`WHERE` de la visibilidad: se usa idéntica en el listado y en el detalle.
fn visible_sql() -> String {
    format!(
        "FROM job_offers o JOIN companies c ON c.id = o.company_id \
         WHERE o.status = 'published' AND o.publication_date IS NOT NULL \
           AND (o.hiring_date IS NULL OR o.hiring_date >= (now() AT TIME ZONE 'UTC')::date) \
           AND {VISIBLE_COMPANY_SQL}"
    )
}

// ------------------------------------------------------------------ consulta

#[derive(Clone, Copy, PartialEq)]
enum Orden {
    Reciente,
    Antiguo,
}

impl Orden {
    fn marca(self) -> char {
        match self {
            Self::Reciente => 'r',
            Self::Antiguo => 'o',
        }
    }
}

fn codificar_cursor(orden: Orden, fecha: &str, id: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("{}|{fecha}|{id}", orden.marca()))
}

/// Un cursor solo sirve para el orden con el que se emitió.
fn decodificar_cursor(c: &str, orden: Orden) -> Option<(String, String)> {
    let texto = String::from_utf8(URL_SAFE_NO_PAD.decode(c).ok()?).ok()?;
    let mut partes = texto.split('|');
    let (marca, fecha, id) = (partes.next()?, partes.next()?, partes.next()?);
    if partes.next().is_some() || marca != orden.marca().to_string() {
        return None;
    }
    input::fecha(fecha)?;
    Some((fecha.to_string(), Uuid::parse_str(id).ok()?.to_string()))
}

struct Consulta {
    limite: i64,
    orden: Orden,
    cursor: Option<(String, String)>,
    /// `(columna, id)` de los filtros presentes.
    filtros: Vec<(&'static str, String)>,
}

fn leer_consulta(raw: Option<&str>) -> Result<Consulta, ApiError> {
    let mut q = parse_consulta(raw, &PERMITIDOS)?;
    let malo = || ApiError::Invalid("La paginación no es válida.");
    let orden = match q.get("sort").map(String::as_str) {
        None | Some("recent") => Orden::Reciente,
        Some("oldest") => Orden::Antiguo,
        // `sort=match` es de la 5b y un `sort` vacío está mal escrito: valores ilegales.
        Some("") | Some("match") => return Err(ApiError::Invalid("El orden pedido no existe.")),
        // Cualquier otro criterio (nombre, empresa, edad...) no existe: no se ordena por él.
        Some(_) => return Err(ApiError::ParamNotAllowed),
    };
    // `Pagina` entiende el cursor genérico (instante e id); aquí el cursor es el propio de este
    // listado (fecha, id y orden), así que se lee aparte.
    let cursor = q.remove("cursor");
    let pagina = Pagina::de(&q)?;
    let cursor = match cursor {
        None => None,
        Some(c) => Some(decodificar_cursor(&c, orden).ok_or_else(malo)?),
    };
    let mut filtros = Vec::new();
    for col in FILTROS {
        if let Some(v) = q.get(col) {
            let id = input::uuid_canonico(v).ok_or(ApiError::Invalid(
                "Un filtro no es un identificador válido.",
            ))?;
            filtros.push((col, id));
        }
    }
    Ok(Consulta {
        limite: pagina.limite,
        orden,
        cursor,
        filtros,
    })
}

fn json_de(texto: String) -> Result<Value, DbErr> {
    serde_json::from_str(&texto).map_err(|e| DbErr::Custom(e.to_string()))
}

// ------------------------------------------------------------------ rutas

/// `GET /offers`. Orden: 401 → 403 cuenta → 413 → 422 de la consulta → 200.
async fn list(
    State(s): State<AppState>,
    RawQuery(raw): RawQuery,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Fallo> {
    cuerpo(body)?;
    let q = leer_consulta(raw.as_deref()).map_err(Fallo::from)?;
    let (cmp, dir) = match q.orden {
        Orden::Reciente => ("<", "DESC"),
        Orden::Antiguo => (">", "ASC"),
    };
    let mut valores: Vec<sea_orm::Value> = vec![
        q.cursor.as_ref().map(|c| c.0.clone()).into(),
        q.cursor.as_ref().map(|c| c.1.clone()).into(),
        (q.limite + 1).into(),
    ];
    let mut filtros = String::new();
    for (col, id) in &q.filtros {
        valores.push(id.clone().into());
        filtros.push_str(&format!(" AND o.{col} = ${}::uuid", valores.len()));
    }
    let sql = format!(
        "SELECT jsonb_build_object({}, 'my_application', NULL)::text, o.publication_date::text, \
                o.id::text \
         {} AND ($1::date IS NULL OR (o.publication_date, o.id) {cmp} ($1::date, $2::uuid)){filtros} \
         ORDER BY o.publication_date {dir}, o.id {dir} LIMIT $3",
        resumen_sql(),
        visible_sql(),
    );
    let filas =
        s.db.query_all(stmt(&sql, valores))
            .await
            .map_err(|e| Fallo::from(db("listar_ofertas")(e)))?;
    let hay_mas = filas.len() as i64 > q.limite;
    let mut items = Vec::new();
    let mut ultimo = None;
    for f in filas.iter().take(q.limite as usize) {
        let leido = (|| -> Result<(Value, String, String), DbErr> {
            Ok((
                json_de(f.try_get_by_index(0)?)?,
                f.try_get_by_index(1)?,
                f.try_get_by_index(2)?,
            ))
        })()
        .map_err(|e| Fallo::from(db("leer_ofertas")(e)))?;
        items.push(leido.0);
        ultimo = Some((leido.1, leido.2));
    }
    let siguiente = match (hay_mas, ultimo) {
        (true, Some((fecha, id))) => Some(codificar_cursor(q.orden, &fecha, &id)),
        _ => None,
    };
    Ok(Json(json!({"items": items, "next_cursor": siguiente})).into_response())
}

/// `GET /offers/{id}`: lo no visible, inexistente o mal formado es el mismo 404.
async fn read(
    State(s): State<AppState>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Fallo> {
    cuerpo(body)?;
    let id = input::uuid_canonico(&id).ok_or(ApiError::NotFound)?;
    let sql = format!(
        "SELECT jsonb_build_object({}, {}, 'my_application', NULL)::text {} AND o.id = $1::uuid",
        resumen_sql(),
        detalle_sql(),
        visible_sql(),
    );
    let fila =
        s.db.query_one(stmt(&sql, vec![id.into()]))
            .await
            .map_err(|e| Fallo::from(db("leer_oferta")(e)))?
            .ok_or(ApiError::NotFound)?;
    let valor = fila
        .try_get_by_index::<String>(0)
        .and_then(json_de)
        .map_err(|e| Fallo::from(db("leer_oferta")(e)))?;
    Ok(Json(valor).into_response())
}

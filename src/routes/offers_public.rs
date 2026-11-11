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
//! - `match`: el puntaje de **quien llama** contra cada oferta, calculado al leer con su propio
//!   perfil (`matching`); `sort=match` ordena por él. Nunca el de otra persona.
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
use std::collections::HashMap;

use sea_orm::{ConnectionTrait, DbErr};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    company_support::{db, parse_consulta, stmt, ApiError, Pagina},
    table::{Access, RouteSpec},
};
use crate::{
    auth_user::AuthUser,
    company_gate::VISIBLE_COMPANY_SQL,
    matching::{self, store as matching_store},
    offers::input,
    state::AppState,
};

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
/// La aplicación **propia** a la oferta (`o`): `{id, status}` o `null`. `$n` es el id de la
/// cuenta de quien llama; nunca se ve la de otra persona.
fn my_application_sql(n: usize) -> String {
    format!(
        "(SELECT jsonb_build_object('id', a.id::text, 'status', a.status::text) \
          FROM job_applications a JOIN applicants ap ON ap.id = a.applicant_id \
          WHERE a.job_offer_id = o.id AND ap.user_id = ${n}::uuid)"
    )
}

pub(crate) fn visible_sql() -> String {
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
    /// Por el puntaje de quien llama (`sort=match`).
    Puntaje,
}

impl Orden {
    fn marca(self) -> char {
        match self {
            Self::Reciente => 'r',
            Self::Antiguo => 'o',
            Self::Puntaje => 'm',
        }
    }

    /// Lo que dice `sorted_by` en la respuesta.
    fn nombre(self) -> &'static str {
        match self {
            Self::Reciente => "recent",
            Self::Antiguo => "oldest",
            Self::Puntaje => "match",
        }
    }
}

/// Dónde quedó la página anterior: `(puntaje si el orden es `match`, fecha, id)`.
type Marca = (Option<u8>, String, String);

fn codificar_cursor(orden: Orden, puntaje: u8, fecha: &str, id: &str) -> String {
    let texto = match orden {
        Orden::Puntaje => format!("m|{puntaje}|{fecha}|{id}"),
        _ => format!("{}|{fecha}|{id}", orden.marca()),
    };
    URL_SAFE_NO_PAD.encode(texto)
}

/// Un cursor solo sirve para el orden con el que se emitió.
fn decodificar_cursor(c: &str, orden: Orden) -> Option<Marca> {
    let texto = String::from_utf8(URL_SAFE_NO_PAD.decode(c).ok()?).ok()?;
    let mut partes = texto.split('|');
    if partes.next()? != orden.marca().to_string() {
        return None;
    }
    let puntaje = match orden {
        Orden::Puntaje => Some(partes.next()?.parse::<u8>().ok().filter(|p| *p <= 100)?),
        _ => None,
    };
    let (fecha, id) = (partes.next()?, partes.next()?);
    if partes.next().is_some() {
        return None;
    }
    input::fecha(fecha)?;
    Some((
        puntaje,
        fecha.to_string(),
        Uuid::parse_str(id).ok()?.to_string(),
    ))
}

struct Consulta {
    limite: i64,
    orden: Orden,
    cursor: Option<Marca>,
    /// `(columna, id)` de los filtros presentes.
    filtros: Vec<(&'static str, String)>,
}

fn leer_consulta(raw: Option<&str>) -> Result<Consulta, ApiError> {
    let mut q = parse_consulta(raw, &PERMITIDOS)?;
    let malo = || ApiError::Invalid("La paginación no es válida.");
    let orden = match q.get("sort").map(String::as_str) {
        None | Some("recent") => Orden::Reciente,
        Some("oldest") => Orden::Antiguo,
        Some("match") => Orden::Puntaje,
        // Un `sort` vacío está mal escrito: valor ilegal.
        Some("") => return Err(ApiError::Invalid("El orden pedido no existe.")),
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

/// Le pone a cada oferta el `match` de quien llama (puntaje y desglose).
fn poner_match(items: &mut [Value], puntajes: &HashMap<String, matching_store::Scored>) {
    for item in items {
        let Some(id) = item["id"].as_str() else {
            continue;
        };
        item["match"] = puntajes.get(id).map_or(Value::Null, |p| p.json.clone());
    }
}

fn fallo_lectura(e: DbErr) -> Fallo {
    Fallo::from(db("leer_ofertas")(e))
}

/// `GET /offers`. Orden: 401 → 403 cuenta → 413 → 422 de la consulta → 200.
async fn list(
    State(s): State<AppState>,
    RawQuery(raw): RawQuery,
    user: AuthUser,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Fallo> {
    cuerpo(body)?;
    let mut q = leer_consulta(raw.as_deref()).map_err(Fallo::from)?;
    let malo = || Fallo::from(ApiError::Invalid("La paginación no es válida."));

    // El perfil es **de quien llama**: de ahí sale todo `match` de esta respuesta.
    let (perfil, hoy) = matching_store::viewer(&s.db, &user.id)
        .await
        .map_err(fallo_lectura)?;

    // Sin perfil no hay con qué ordenar por puntaje: se ordena por fecha y se dice.
    let mut no_disponible = Value::Null;
    if q.orden == Orden::Puntaje && perfil.is_empty() {
        if q.cursor.is_some() {
            return Err(malo());
        }
        q.orden = Orden::Reciente;
        no_disponible = json!("perfil_vacio");
    }

    let (items, siguiente) = if q.orden == Orden::Puntaje {
        por_puntaje(&s, &user, &q, &perfil, hoy).await?
    } else {
        por_fecha(&s, &user, &q, &perfil, hoy).await?
    };
    Ok(Json(json!({
        "items": items,
        "next_cursor": siguiente,
        "sorted_by": q.orden.nombre(),
        "match_unavailable": no_disponible,
    }))
    .into_response())
}

/// Los valores y los `AND o.col = $n` de los filtros, a partir de `$desde`.
fn filtros_sql(q: &Consulta, valores: &mut Vec<sea_orm::Value>) -> String {
    let mut filtros = String::new();
    for (col, id) in &q.filtros {
        valores.push(id.clone().into());
        filtros.push_str(&format!(" AND o.{col} = ${}::uuid", valores.len()));
    }
    filtros
}

/// `sort=recent|oldest`: la página sale de la base, y cada oferta lleva su `match`.
async fn por_fecha(
    s: &AppState,
    user: &AuthUser,
    q: &Consulta,
    perfil: &matching::Profile,
    hoy: matching::Ymd,
) -> Result<(Vec<Value>, Option<String>), Fallo> {
    let (cmp, dir) = match q.orden {
        Orden::Antiguo => (">", "ASC"),
        _ => ("<", "DESC"),
    };
    let mut valores: Vec<sea_orm::Value> = vec![
        q.cursor.as_ref().map(|c| c.1.clone()).into(),
        q.cursor.as_ref().map(|c| c.2.clone()).into(),
        (q.limite + 1).into(),
    ];
    let filtros = filtros_sql(q, &mut valores);
    valores.push(user.id.clone().into());
    let mia = my_application_sql(valores.len());
    let sql = format!(
        "SELECT jsonb_build_object({}, 'my_application', {mia})::text, o.publication_date::text, \
                o.id::text \
         {} AND ($1::date IS NULL OR (o.publication_date, o.id) {cmp} ($1::date, $2::uuid)){filtros} \
         ORDER BY o.publication_date {dir}, o.id {dir} LIMIT $3",
        resumen_sql(),
        visible_sql(),
    );
    let filas =
        s.db.query_all(stmt(&sql, valores))
            .await
            .map_err(fallo_lectura)?;
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
        .map_err(fallo_lectura)?;
        items.push(leido.0);
        ultimo = Some((leido.1, leido.2));
    }
    let ids: Vec<String> = items
        .iter()
        .filter_map(|i| i["id"].as_str().map(str::to_string))
        .collect();
    let reqs = matching_store::requirements(&s.db, &ids)
        .await
        .map_err(fallo_lectura)?;
    poner_match(&mut items, &matching_store::score_all(perfil, &reqs, hoy));
    let siguiente = match (hay_mas, ultimo) {
        (true, Some((fecha, id))) => Some(codificar_cursor(q.orden, 0, &fecha, &id)),
        _ => None,
    };
    Ok((items, siguiente))
}

/// `sort=match`: el puntaje se calcula para **todas** las ofertas visibles (con los filtros) y
/// se ordena en el servidor: puntaje, fecha e id, descendentes. El cursor guarda el último
/// `(puntaje, fecha, id)` entregado, así que una oferta que cambia entre páginas no repite ni
/// salta a las demás.
async fn por_puntaje(
    s: &AppState,
    user: &AuthUser,
    q: &Consulta,
    perfil: &matching::Profile,
    hoy: matching::Ymd,
) -> Result<(Vec<Value>, Option<String>), Fallo> {
    let mut valores: Vec<sea_orm::Value> = vec![];
    let filtros = filtros_sql(q, &mut valores);
    let sql = format!(
        "SELECT o.id::text, o.publication_date::text {}{filtros}",
        visible_sql()
    );
    let filas =
        s.db.query_all(stmt(&sql, valores))
            .await
            .map_err(fallo_lectura)?;
    let mut todas: Vec<(String, String)> = Vec::with_capacity(filas.len());
    for f in &filas {
        todas.push((
            f.try_get_by_index(0).map_err(fallo_lectura)?,
            f.try_get_by_index(1).map_err(fallo_lectura)?,
        ));
    }
    let ids: Vec<String> = todas.iter().map(|t| t.0.clone()).collect();
    let reqs = matching_store::requirements(&s.db, &ids)
        .await
        .map_err(fallo_lectura)?;
    let mut puntajes = matching_store::score_all(perfil, &reqs, hoy);

    // (puntaje, fecha, id) de mayor a menor.
    let mut orden: Vec<(u8, String, String)> = todas
        .into_iter()
        .filter_map(|(id, fecha)| puntajes.get(&id).map(|p| (p.score, fecha, id)))
        .collect();
    orden.sort_unstable_by(|a, b| b.cmp(a));
    if let Some((ps, pf, pi)) = &q.cursor {
        let ancla = (ps.unwrap_or(0), pf.clone(), pi.clone());
        orden.retain(|t| *t < ancla);
    }
    let hay_mas = orden.len() as i64 > q.limite;
    orden.truncate(q.limite as usize);
    let pagina: Vec<String> = orden.iter().map(|t| t.2.clone()).collect();
    let siguiente = match (hay_mas, orden.last()) {
        (true, Some((p, f, i))) => Some(codificar_cursor(Orden::Puntaje, *p, f, i)),
        _ => None,
    };
    if pagina.is_empty() {
        return Ok((vec![], None));
    }

    let sql = format!(
        "SELECT jsonb_build_object({}, 'my_application', {})::text, o.id::text {} \
         AND o.id = ANY (string_to_array($1, ',')::uuid[])",
        resumen_sql(),
        my_application_sql(2),
        visible_sql(),
    );
    let filas =
        s.db.query_all(stmt(
            &sql,
            vec![pagina.join(",").into(), user.id.clone().into()],
        ))
        .await
        .map_err(fallo_lectura)?;
    let mut por_id: HashMap<String, Value> = HashMap::new();
    for f in &filas {
        let valor = f
            .try_get_by_index::<String>(0)
            .and_then(json_de)
            .map_err(fallo_lectura)?;
        por_id.insert(f.try_get_by_index(1).map_err(fallo_lectura)?, valor);
    }
    // Una oferta que dejó de ser visible entre las dos consultas simplemente no sale.
    let mut items: Vec<Value> = pagina.iter().filter_map(|id| por_id.remove(id)).collect();
    puntajes.retain(|id, _| pagina.contains(id));
    poner_match(&mut items, &puntajes);
    Ok((items, siguiente))
}

/// `GET /offers/{id}`: lo no visible, inexistente o mal formado es el mismo 404.
async fn read(
    State(s): State<AppState>,
    Path(id): Path<String>,
    user: AuthUser,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Fallo> {
    cuerpo(body)?;
    let id = input::uuid_canonico(&id).ok_or(ApiError::NotFound)?;
    let mia = my_application_sql(2);
    let sql = format!(
        "SELECT jsonb_build_object({}, {}, 'my_application', {mia})::text {} AND o.id = $1::uuid",
        resumen_sql(),
        detalle_sql(),
        visible_sql(),
    );
    let fila =
        s.db.query_one(stmt(&sql, vec![id.clone().into(), user.id.clone().into()]))
            .await
            .map_err(|e| Fallo::from(db("leer_oferta")(e)))?
            .ok_or(ApiError::NotFound)?;
    let valor = fila
        .try_get_by_index::<String>(0)
        .and_then(json_de)
        .map_err(|e| Fallo::from(db("leer_oferta")(e)))?;
    let mut valor = valor;
    let (perfil, hoy) = matching_store::viewer(&s.db, &user.id)
        .await
        .map_err(fallo_lectura)?;
    let reqs = matching_store::requirements(&s.db, std::slice::from_ref(&id))
        .await
        .map_err(fallo_lectura)?;
    poner_match(
        std::slice::from_mut(&mut valor),
        &matching_store::score_all(&perfil, &reqs, hoy),
    );
    Ok(Json(valor).into_response())
}

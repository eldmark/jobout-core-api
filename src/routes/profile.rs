//! `GET/PUT /api/v1/profile` y `GET /api/v1/cvs/{id}/extraction[/confirm]`: el perfil de la
//! persona y la revisión de lo que se extrajo de su CV.
//!
//! No existe ruta por id de perfil: el dueño sale siempre del token (`AuthUser`). El CV sí
//! viaja por id, pero solo se alcanza si pertenece a quien pregunta; si no, 404 (S1).
//!
//! `PUT /profile` y `POST /cvs/{id}/extraction/confirm` comparten **un único** módulo de
//! validación y guardado: el confirm guarda lo que la persona envió (nunca lo propuesto) con las
//! mismas reglas que el PUT. Todo va en una transacción.
//!
//! Cifrado: `personal_data.phone` y `applicant_experiences.description` se guardan cifrados.
//! Los mensajes de error son textos fijos: nunca repiten un dato recibido ni el texto de la base.

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json,
};
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, Statement, TransactionTrait, Value,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json_};
use std::collections::HashSet;

use super::table::{Access, RouteSpec};
use crate::{auth_user::AuthUser, state::AppState};

pub fn routes() -> Vec<RouteSpec> {
    vec![
        RouteSpec {
            path: "/api/v1/profile",
            methods: &["GET", "PUT"],
            access: Access::Authenticated,
            body_limit: None,
            handler: get(read_profile).put(put_profile),
        },
        RouteSpec {
            path: "/api/v1/cvs/{id}/extraction",
            methods: &["GET"],
            access: Access::Authenticated,
            body_limit: None,
            handler: get(read_extraction),
        },
        RouteSpec {
            path: "/api/v1/cvs/{id}/extraction/confirm",
            methods: &["POST"],
            access: Access::Authenticated,
            body_limit: None,
            handler: post(confirm_extraction),
        },
    ]
}

// ------------------------------------------------------------------ errores

const MAX_SKILLS: usize = 60;
const MAX_LANGUAGES: usize = 10;
const MAX_EXPERIENCES: usize = 30;
const MAX_TEXT: usize = 2000;
const MAX_SHORT: usize = 200;
const MAX_PHONE: usize = 20;
const MAX_YEARS: i32 = 80;

enum ApiError {
    NotFound,
    /// Texto fijo: jamás un dato recibido.
    Invalid(&'static str),
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
            Self::Invalid(mensaje) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "datos_invalidos", mensaje)
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

/// Registra el fallo por categoría, **nunca** con el texto del error de la base.
fn db(donde: &'static str) -> impl Fn(DbErr) -> ApiError {
    move |_| {
        tracing::error!(donde, "error interno");
        ApiError::Internal
    }
}

fn interno(donde: &'static str) -> ApiError {
    tracing::error!(donde, "error interno");
    ApiError::Internal
}

fn stmt(sql: &str, valores: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, valores)
}

// ------------------------------------------------------------------ formato

#[derive(Deserialize, Serialize, Default)]
struct SkillIn {
    skill_id: String,
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    years_of_experience: Option<i32>,
}

#[derive(Deserialize, Serialize)]
struct LanguageIn {
    language_id: String,
    proficiency: String,
}

#[derive(Deserialize, Serialize)]
struct ExperienceIn {
    company: String,
    title: String,
    start_date: String,
    #[serde(default)]
    end_date: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// Lo que comparten el PUT y el confirm. Lo que no se envía se vacía.
#[derive(Deserialize, Default)]
struct Core {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    skills: Vec<SkillIn>,
    #[serde(default)]
    languages: Vec<LanguageIn>,
    #[serde(default)]
    experiences: Vec<ExperienceIn>,
}

#[derive(Deserialize)]
struct PutIn {
    #[serde(default)]
    phone: Option<String>,
    #[serde(default)]
    academic_level_id: Option<String>,
    #[serde(default)]
    profession_id: Option<String>,
    #[serde(default)]
    job_hierarchy_id: Option<String>,
    #[serde(default)]
    extra_knowledge: Option<String>,
    #[serde(flatten)]
    core: Core,
}

/// El cuerpo se lee a mano para que un JSON mal formado dé nuestro 422 con texto fijo, no el
/// mensaje del deserializador (que puede citar el dato).
fn parse<T: serde::de::DeserializeOwned>(cuerpo: &Bytes) -> Result<T, ApiError> {
    serde_json::from_slice(cuerpo)
        .map_err(|_| ApiError::Invalid("El cuerpo no tiene el formato esperado."))
}

// -------------------------------------------------------------- validación

/// UUID en texto canónico (8-4-4-4-12 hexadecimal) → minúsculas. Es lo que evita que un id
/// malformado llegue al SQL.
pub(crate) fn uuid_canonico(s: &str) -> Option<String> {
    let partes: Vec<&str> = s.split('-').collect();
    let largos = [8, 4, 4, 4, 12];
    let ok = partes.len() == 5
        && partes
            .iter()
            .zip(largos)
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()));
    ok.then(|| s.to_ascii_lowercase())
}

/// `AAAA-MM-DD` real (con años bisiestos). Devuelve la tupla para poder comparar fechas.
pub(crate) fn fecha(s: &str) -> Option<(i32, u32, u32)> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = &s[r];
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    let (y, m, d) = (num(0..4)? as i32, num(5..7)?, num(8..10)?);
    let bisiesto = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dias = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if bisiesto => 29,
        2 => 28,
        _ => return None,
    };
    (y >= 1900 && (1..=dias).contains(&d)).then_some((y, m, d))
}

/// Texto libre: recortado, vacío = ausente, tope de 2000 caracteres. El HTML se guarda tal cual:
/// es texto, nunca se interpreta.
fn texto(t: &Option<String>, max: usize) -> Result<Option<String>, ApiError> {
    match t.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) if s.chars().count() > max => Err(ApiError::Invalid(
            "Un texto es demasiado largo (máximo 2000 caracteres).",
        )),
        Some(s) => Ok(Some(s.to_string())),
    }
}

struct CoreOk {
    description: Option<String>,
    skills: Vec<(String, Option<String>, Option<i32>)>,
    languages: Vec<(String, String)>,
    experiences: Vec<ExperienceOk>,
}

/// Una experiencia ya validada: fechas canónicas, textos recortados.
struct ExperienceOk {
    company: String,
    title: String,
    start: String,
    end: Option<String>,
    description: Option<String>,
}

fn validar_core(c: &Core) -> Result<CoreOk, ApiError> {
    if c.skills.len() > MAX_SKILLS {
        return Err(ApiError::Invalid("Hay demasiadas skills (máximo 60)."));
    }
    if c.languages.len() > MAX_LANGUAGES {
        return Err(ApiError::Invalid("Hay demasiados idiomas (máximo 10)."));
    }
    if c.experiences.len() > MAX_EXPERIENCES {
        return Err(ApiError::Invalid(
            "Hay demasiadas experiencias (máximo 30).",
        ));
    }
    let description = texto(&c.description, MAX_TEXT)?;

    let mut vistos = HashSet::new();
    let mut skills = Vec::new();
    for s in &c.skills {
        let id = uuid_canonico(&s.skill_id).ok_or(ApiError::Invalid("Una skill no es válida."))?;
        if !vistos.insert(id.clone()) {
            return Err(ApiError::Invalid("Hay una skill repetida."));
        }
        if let Some(n) = &s.level {
            if !["beginner", "intermediate", "advanced", "expert"].contains(&n.as_str()) {
                return Err(ApiError::Invalid("Un nivel de skill no es válido."));
            }
        }
        if s.years_of_experience
            .is_some_and(|y| !(0..=MAX_YEARS).contains(&y))
        {
            return Err(ApiError::Invalid(
                "Unos años de experiencia no son válidos.",
            ));
        }
        skills.push((id, s.level.clone(), s.years_of_experience));
    }

    let mut vistos = HashSet::new();
    let mut languages = Vec::new();
    for l in &c.languages {
        let id =
            uuid_canonico(&l.language_id).ok_or(ApiError::Invalid("Un idioma no es válido."))?;
        if !vistos.insert(id.clone()) {
            return Err(ApiError::Invalid("Hay un idioma repetido."));
        }
        if !["native", "advanced", "intermediate", "basic"].contains(&l.proficiency.as_str()) {
            return Err(ApiError::Invalid("Un nivel de idioma no es válido."));
        }
        languages.push((id, l.proficiency.clone()));
    }

    let mut experiences = Vec::new();
    for e in &c.experiences {
        let company = e.company.trim();
        let title = e.title.trim();
        if company.is_empty()
            || title.is_empty()
            || company.chars().count() > MAX_SHORT
            || title.chars().count() > MAX_SHORT
        {
            return Err(ApiError::Invalid(
                "Una experiencia tiene empresa o cargo no válidos.",
            ));
        }
        let inicio =
            fecha(&e.start_date).ok_or(ApiError::Invalid("Una fecha de inicio no es válida."))?;
        let fin = match e.end_date.as_deref() {
            None => None,
            Some(f) => Some((
                fecha(f).ok_or(ApiError::Invalid("Una fecha de fin no es válida."))?,
                f,
            )),
        };
        if fin.is_some_and(|(f, _)| f < inicio) {
            return Err(ApiError::Invalid(
                "Una experiencia termina antes de empezar.",
            ));
        }
        experiences.push(ExperienceOk {
            company: company.to_string(),
            title: title.to_string(),
            start: e.start_date.clone(),
            end: fin.map(|(_, f)| f.to_string()),
            description: texto(&e.description, MAX_TEXT)?,
        });
    }
    Ok(CoreOk {
        description,
        skills,
        languages,
        experiences,
    })
}

fn telefono(t: &Option<String>) -> Result<Option<String>, ApiError> {
    match t.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(s)
            if s.chars().count() <= MAX_PHONE
                && s.chars().all(|c| c.is_ascii_digit() || "+ ()-".contains(c)) =>
        {
            Ok(Some(s.to_string()))
        }
        Some(_) => Err(ApiError::Invalid("El teléfono no es válido.")),
    }
}

fn id_opcional(t: &Option<String>) -> Result<Option<String>, ApiError> {
    match t {
        None => Ok(None),
        Some(s) => uuid_canonico(s)
            .map(Some)
            .ok_or(ApiError::Invalid("Un dato del catálogo no es válido.")),
    }
}

/// Todos los ids existen **y están activos** en su catálogo. `tabla` es siempre una constante.
async fn existen(
    txn: &DatabaseTransaction,
    tabla: &'static str,
    ids: &[String],
) -> Result<bool, ApiError> {
    if ids.is_empty() {
        return Ok(true);
    }
    let fila = txn
        .query_one(stmt(
            &format!("SELECT count(*) FROM {tabla} WHERE active AND id = ANY($1::text[]::uuid[])"),
            vec![ids.to_vec().into()],
        ))
        .await
        .map_err(db("validar_catalogo"))?
        .ok_or_else(|| interno("validar_catalogo_fila"))?;
    let n: i64 = fila.try_get_by_index(0).map_err(db("validar_catalogo_n"))?;
    Ok(n as usize == ids.len())
}

async fn validar_catalogos(
    txn: &DatabaseTransaction,
    core: &CoreOk,
    extra: &[(&'static str, Option<&String>)],
) -> Result<(), ApiError> {
    let skills: Vec<String> = core.skills.iter().map(|s| s.0.clone()).collect();
    let idiomas: Vec<String> = core.languages.iter().map(|l| l.0.clone()).collect();
    if !existen(txn, "skills", &skills).await? {
        return Err(ApiError::Invalid("Una skill no existe."));
    }
    if !existen(txn, "languages", &idiomas).await? {
        return Err(ApiError::Invalid("Un idioma no existe."));
    }
    for (tabla, id) in extra {
        if let Some(id) = id {
            if !existen(txn, tabla, std::slice::from_ref(*id)).await? {
                return Err(ApiError::Invalid("Un dato del catálogo no existe."));
            }
        }
    }
    Ok(())
}

// ----------------------------------------------------------------- guardado

async fn applicant_de(txn: &DatabaseTransaction, user_id: &str) -> Result<String, ApiError> {
    let fila = txn
        .query_one(stmt(
            "SELECT id::text FROM applicants WHERE user_id = $1::uuid",
            vec![user_id.to_string().into()],
        ))
        .await
        .map_err(db("leer_applicant"))?
        .ok_or_else(|| interno("sin_applicant"))?;
    fila.try_get_by_index(0).map_err(db("leer_applicant_id"))
}

/// Reemplaza skills, idiomas y experiencias y la descripción. Es lo que guardan **ambas** rutas.
/// La descripción de cada experiencia se cifra aquí, en un solo lugar.
async fn guardar_core(
    txn: &DatabaseTransaction,
    state: &AppState,
    applicant: &str,
    c: &CoreOk,
) -> Result<(), ApiError> {
    let ap = || -> Value { applicant.to_string().into() };
    txn.execute(stmt(
        "INSERT INTO professional_data (id, applicant_id, description) \
         VALUES ($1::uuid, $2::uuid, $3) \
         ON CONFLICT (applicant_id) DO UPDATE SET description = EXCLUDED.description, \
             updated_at = now()",
        vec![
            migration::new_id().to_string().into(),
            ap(),
            c.description.clone().into(),
        ],
    ))
    .await
    .map_err(db("guardar_descripcion"))?;

    for tabla in [
        "applicant_skills",
        "applicant_languages",
        "applicant_experiences",
    ] {
        txn.execute(stmt(
            &format!("DELETE FROM {tabla} WHERE applicant_id = $1::uuid"),
            vec![ap()],
        ))
        .await
        .map_err(db("vaciar_listas"))?;
    }
    for (id, level, years) in &c.skills {
        txn.execute(stmt(
            "INSERT INTO applicant_skills (id, applicant_id, skill_id, level, years_of_experience) \
             VALUES ($1::uuid, $2::uuid, $3::uuid, $4::skill_level, $5)",
            vec![
                migration::new_id().to_string().into(),
                ap(),
                id.clone().into(),
                level.clone().into(),
                (*years).into(),
            ],
        ))
        .await
        .map_err(db("guardar_skill"))?;
    }
    for (id, prof) in &c.languages {
        txn.execute(stmt(
            "INSERT INTO applicant_languages (id, applicant_id, language_id, proficiency) \
             VALUES ($1::uuid, $2::uuid, $3::uuid, $4::proficiency)",
            vec![
                migration::new_id().to_string().into(),
                ap(),
                id.clone().into(),
                prof.clone().into(),
            ],
        ))
        .await
        .map_err(db("guardar_idioma"))?;
    }
    for (pos, e) in c.experiences.iter().enumerate() {
        let cifrada = match &e.description {
            Some(d) => Some(
                state
                    .pii
                    .encrypt(d)
                    .map_err(|_| interno("cifrar_experiencia"))?,
            ),
            None => None,
        };
        txn.execute(stmt(
            "INSERT INTO applicant_experiences \
                 (id, applicant_id, company, title, start_date, end_date, description, position) \
             VALUES ($1::uuid, $2::uuid, $3, $4, $5::date, $6::date, $7, $8)",
            vec![
                migration::new_id().to_string().into(),
                ap(),
                e.company.clone().into(),
                e.title.clone().into(),
                e.start.clone().into(),
                e.end.clone().into(),
                cifrada.into(),
                (pos as i32).into(),
            ],
        ))
        .await
        .map_err(db("guardar_experiencia"))?;
    }
    Ok(())
}

async fn put_profile(
    State(state): State<AppState>,
    user: AuthUser,
    cuerpo: Bytes,
) -> Result<StatusCode, ApiError> {
    let entrada: PutIn = parse(&cuerpo)?;
    let core = validar_core(&entrada.core)?;
    let telefono = telefono(&entrada.phone)?;
    let extra = texto(&entrada.extra_knowledge, MAX_TEXT)?;
    let nivel = id_opcional(&entrada.academic_level_id)?;
    let profesion = id_opcional(&entrada.profession_id)?;
    let jerarquia = id_opcional(&entrada.job_hierarchy_id)?;
    let cifrado = match &telefono {
        Some(t) => Some(
            state
                .pii
                .encrypt(t)
                .map_err(|_| interno("cifrar_telefono"))?,
        ),
        None => None,
    };

    let txn = state.db.begin().await.map_err(db("begin"))?;
    let applicant = applicant_de(&txn, &user.id).await?;
    validar_catalogos(
        &txn,
        &core,
        &[
            ("academic_levels", nivel.as_ref()),
            ("professions", profesion.as_ref()),
            ("job_hierarchies", jerarquia.as_ref()),
        ],
    )
    .await?;
    guardar_core(&txn, &state, &applicant, &core).await?;

    txn.execute(stmt(
        "UPDATE professional_data SET last_academic_level_id = $2::uuid, \
             profession_id = $3::uuid, last_job_hierarchy_id = $4::uuid \
         WHERE applicant_id = $1::uuid",
        vec![
            applicant.clone().into(),
            nivel.into(),
            profesion.into(),
            jerarquia.into(),
        ],
    ))
    .await
    .map_err(db("guardar_profesional"))?;
    txn.execute(stmt(
        "INSERT INTO personal_data (id, applicant_id, phone) VALUES ($1::uuid, $2::uuid, $3) \
         ON CONFLICT (applicant_id) DO UPDATE SET phone = EXCLUDED.phone, updated_at = now()",
        vec![
            migration::new_id().to_string().into(),
            applicant.clone().into(),
            cifrado.into(),
        ],
    ))
    .await
    .map_err(db("guardar_telefono"))?;
    txn.execute(stmt(
        "DELETE FROM applicant_extra_knowledge WHERE applicant_id = $1::uuid",
        vec![applicant.clone().into()],
    ))
    .await
    .map_err(db("vaciar_extra"))?;
    if let Some(e) = extra {
        txn.execute(stmt(
            "INSERT INTO applicant_extra_knowledge (id, applicant_id, description) \
             VALUES ($1::uuid, $2::uuid, $3)",
            vec![
                migration::new_id().to_string().into(),
                applicant.into(),
                e.into(),
            ],
        ))
        .await
        .map_err(db("guardar_extra"))?;
    }
    txn.commit().await.map_err(db("commit"))?;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------ lectura

async fn read_profile(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Json_>, ApiError> {
    let uid = || -> Vec<Value> { vec![user.id.clone().into()] };
    let a = "(SELECT id FROM applicants WHERE user_id = $1::uuid)";

    let prof = state
        .db
        .query_one(stmt(
            &format!(
                "SELECT description, last_academic_level_id::text, profession_id::text, \
                        last_job_hierarchy_id::text \
                 FROM professional_data WHERE applicant_id = {a}"
            ),
            uid(),
        ))
        .await
        .map_err(db("leer_profesional"))?;
    let (descripcion, nivel, profesion, jerarquia): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = match prof {
        Some(f) => (
            f.try_get_by_index(0).map_err(db("leer_profesional"))?,
            f.try_get_by_index(1).map_err(db("leer_profesional"))?,
            f.try_get_by_index(2).map_err(db("leer_profesional"))?,
            f.try_get_by_index(3).map_err(db("leer_profesional"))?,
        ),
        None => (None, None, None, None),
    };

    let telefono = match state
        .db
        .query_one(stmt(
            &format!("SELECT phone FROM personal_data WHERE applicant_id = {a}"),
            uid(),
        ))
        .await
        .map_err(db("leer_telefono"))?
    {
        Some(f) => {
            let c: Option<Vec<u8>> = f.try_get_by_index(0).map_err(db("leer_telefono"))?;
            match c {
                Some(c) => Some(
                    state
                        .pii
                        .decrypt(&c)
                        .map_err(|_| interno("descifrar_telefono"))?,
                ),
                None => None,
            }
        }
        None => None,
    };

    let extra: Option<String> = match state
        .db
        .query_one(stmt(
            &format!("SELECT description FROM applicant_extra_knowledge WHERE applicant_id = {a}"),
            uid(),
        ))
        .await
        .map_err(db("leer_extra"))?
    {
        Some(f) => Some(f.try_get_by_index(0).map_err(db("leer_extra"))?),
        None => None,
    };

    let mut skills = Vec::new();
    for f in state
        .db
        .query_all(stmt(
            &format!(
                "SELECT skill_id::text, level::text, years_of_experience \
                 FROM applicant_skills WHERE applicant_id = {a} ORDER BY created_at, id"
            ),
            uid(),
        ))
        .await
        .map_err(db("leer_skills"))?
    {
        skills.push(SkillIn {
            skill_id: f.try_get_by_index(0).map_err(db("leer_skills"))?,
            level: f.try_get_by_index(1).map_err(db("leer_skills"))?,
            years_of_experience: f.try_get_by_index(2).map_err(db("leer_skills"))?,
        });
    }
    let mut languages = Vec::new();
    for f in state
        .db
        .query_all(stmt(
            &format!(
                "SELECT language_id::text, proficiency::text \
                 FROM applicant_languages WHERE applicant_id = {a} ORDER BY created_at, id"
            ),
            uid(),
        ))
        .await
        .map_err(db("leer_idiomas"))?
    {
        languages.push(LanguageIn {
            language_id: f.try_get_by_index(0).map_err(db("leer_idiomas"))?,
            proficiency: f.try_get_by_index(1).map_err(db("leer_idiomas"))?,
        });
    }
    let mut experiences = Vec::new();
    for f in state
        .db
        .query_all(stmt(
            &format!(
                "SELECT company, title, start_date::text, end_date::text, description \
                 FROM applicant_experiences WHERE applicant_id = {a} ORDER BY position, id"
            ),
            uid(),
        ))
        .await
        .map_err(db("leer_experiencias"))?
    {
        let cifrada: Option<Vec<u8>> = f.try_get_by_index(4).map_err(db("leer_experiencias"))?;
        experiences.push(ExperienceIn {
            company: f.try_get_by_index(0).map_err(db("leer_experiencias"))?,
            title: f.try_get_by_index(1).map_err(db("leer_experiencias"))?,
            start_date: f.try_get_by_index(2).map_err(db("leer_experiencias"))?,
            end_date: f.try_get_by_index(3).map_err(db("leer_experiencias"))?,
            description: match cifrada {
                Some(c) => Some(
                    state
                        .pii
                        .decrypt(&c)
                        .map_err(|_| interno("descifrar_experiencia"))?,
                ),
                None => None,
            },
        });
    }

    Ok(Json(json!({
        "description": descripcion,
        "phone": telefono,
        "academic_level_id": nivel,
        "profession_id": profesion,
        "job_hierarchy_id": jerarquia,
        "skills": skills,
        "languages": languages,
        "experiences": experiences,
        "extra_knowledge": extra,
    })))
}

// --------------------------------------------------------------- extracción

/// El CV existe **y es de quien pregunta**; si no, 404 (S1). Un id malformado también es 404.
async fn cv_propio(
    exec: &impl ConnectionTrait,
    user_id: &str,
    id: &str,
) -> Result<String, ApiError> {
    let id = uuid_canonico(id).ok_or(ApiError::NotFound)?;
    let fila = exec
        .query_one(stmt(
            "SELECT c.id::text FROM cvs c JOIN applicants a ON a.id = c.applicant_id \
             WHERE c.id = $1::uuid AND a.user_id = $2::uuid",
            vec![id.into(), user_id.to_string().into()],
        ))
        .await
        .map_err(db("leer_cv"))?
        .ok_or(ApiError::NotFound)?;
    fila.try_get_by_index(0).map_err(db("leer_cv_id"))
}

async fn read_extraction(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Json_>, ApiError> {
    let cv = cv_propio(&state.db, &user.id, &id).await?;
    let fila = state
        .db
        .query_one(stmt(
            "SELECT status::text, reason, extracted, reviewed_by_applicant \
             FROM cv_extractions WHERE cv_id = $1::uuid",
            vec![cv.into()],
        ))
        .await
        .map_err(db("leer_extraccion"))?
        .ok_or(ApiError::NotFound)?;
    let estado: String = fila.try_get_by_index(0).map_err(db("leer_extraccion"))?;
    let motivo: Option<String> = fila.try_get_by_index(1).map_err(db("leer_extraccion"))?;
    let extraido: Json_ = fila.try_get_by_index(2).map_err(db("leer_extraccion"))?;
    let revisada: bool = fila.try_get_by_index(3).map_err(db("leer_extraccion"))?;
    let propuesta = if estado == "ready" {
        extraido
    } else {
        Json_::Null
    };
    Ok(Json(json!({
        "status": estado,
        "reason": motivo,
        "proposal": propuesta,
        "reviewed": revisada,
    })))
}

async fn confirm_extraction(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    cuerpo: Bytes,
) -> Result<StatusCode, ApiError> {
    // El 404 va antes que el 422: no se revela nada de un CV ajeno validando su cuerpo.
    let txn = state.db.begin().await.map_err(db("begin"))?;
    let cv = cv_propio(&txn, &user.id, &id).await?;
    let entrada: Core = parse(&cuerpo)?;
    let core = validar_core(&entrada)?;
    let applicant = applicant_de(&txn, &user.id).await?;
    validar_catalogos(&txn, &core, &[]).await?;
    guardar_core(&txn, &state, &applicant, &core).await?;
    let marcadas = txn
        .execute(stmt(
            "UPDATE cv_extractions SET reviewed_by_applicant = true, updated_at = now() \
             WHERE cv_id = $1::uuid",
            vec![cv.into()],
        ))
        .await
        .map_err(db("marcar_revisada"))?
        .rows_affected();
    if marcadas != 1 {
        return Err(ApiError::NotFound);
    }
    txn.commit().await.map_err(db("commit"))?;
    Ok(StatusCode::NO_CONTENT)
}

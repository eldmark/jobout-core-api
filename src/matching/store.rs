//! SQL del matching. Solo **lee**: nada de esto escribe una fila (el puntaje se calcula al
//! leer, ver `api.md`). Todo valor viaja como parámetro.
//!
//! Lo que se lee de la persona es lo que `Profile` admite: habilidades, idiomas, periodos de
//! experiencia y el orden de su nivel académico. Ninguna otra columna suya se consulta aquí.

use std::collections::HashMap;

use sea_orm::{ConnectionTrait, DbBackend, DbErr, QueryResult, Statement, Value};
use serde_json::Value as Json;

use super::{
    compute, Cat, Data, Level, Period, Proficiency, Profile, ReqLanguage, ReqSkill, Requirements,
    Ymd,
};
use crate::offers::input::fecha;

fn stmt(sql: &str, valores: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, valores)
}

fn col<T: sea_orm::TryGetable>(r: &QueryResult, i: usize) -> Result<T, DbErr> {
    r.try_get_by_index(i)
}

fn fecha_de(s: &str) -> Result<Ymd, DbErr> {
    fecha(s).ok_or_else(|| DbErr::Custom("fecha ilegible".into()))
}

/// «Hoy» (fecha UTC de la base): el único reloj del matching.
pub async fn today<C: ConnectionTrait>(db: &C) -> Result<Ymd, DbErr> {
    let f = db
        .query_one(stmt(
            "SELECT (now() AT TIME ZONE 'UTC')::date::text",
            vec![],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("sin fila".into()))?;
    fecha_de(&col::<String>(&f, 0)?)
}

/// El perfil **propio** de una cuenta, completo, y «hoy», en **una** consulta. Sin perfil (o sin
/// fila de persona) es vacío.
pub async fn viewer<C: ConnectionTrait>(db: &C, user_id: &str) -> Result<(Profile, Ymd), DbErr> {
    let f = db
        .query_one(stmt(
            "SELECT (now() AT TIME ZONE 'UTC')::date::text, \
                (SELECT COALESCE(jsonb_agg(jsonb_build_object('id', k.skill_id, 'level', k.level::text)), '[]'::jsonb) \
                 FROM applicant_skills k WHERE k.applicant_id = a.id)::text, \
                (SELECT COALESCE(jsonb_agg(jsonb_build_object('id', l.language_id, 'level', l.proficiency::text)), '[]'::jsonb) \
                 FROM applicant_languages l WHERE l.applicant_id = a.id)::text, \
                (SELECT COALESCE(jsonb_agg(jsonb_build_object('start', e.start_date::text, 'end', e.end_date::text)), '[]'::jsonb) \
                 FROM applicant_experiences e WHERE e.applicant_id = a.id)::text, \
                (SELECT al.sort_order FROM professional_data d \
                 JOIN academic_levels al ON al.id = d.last_academic_level_id \
                 WHERE d.applicant_id = a.id) \
             FROM (SELECT 1) x LEFT JOIN applicants a ON a.user_id = $1::uuid",
            vec![user_id.to_string().into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("sin fila".into()))?;
    let hoy = fecha_de(&col::<String>(&f, 0)?)?;
    let lista = |i: usize| -> Result<Vec<Json>, DbErr> {
        let t: String = col(&f, i)?;
        serde_json::from_str(&t).map_err(|_| DbErr::Custom("json ilegible".into()))
    };
    let texto = |v: &Json, k: &str| v.get(k).and_then(Json::as_str).map(str::to_string);
    let skills = lista(1)?
        .iter()
        .filter_map(|v| {
            Some((
                texto(v, "id")?,
                texto(v, "level").as_deref().and_then(Level::parse),
            ))
        })
        .collect();
    let languages = lista(2)?
        .iter()
        .filter_map(|v| Some((texto(v, "id")?, Proficiency::parse(&texto(v, "level")?)?)))
        .collect();
    let experience = lista(3)?
        .iter()
        .filter_map(|v| {
            Some(Period {
                start: fecha(&texto(v, "start")?)?,
                end: texto(v, "end").as_deref().and_then(fecha),
            })
        })
        .collect();
    Ok((
        Profile {
            skills: Data::Known(skills),
            languages: Data::Known(languages),
            experience: Data::Known(experience),
            academic_sort: Data::Known(col::<Option<i32>>(&f, 4)?),
        },
        hoy,
    ))
}

/// `sort_order` de cada nivel académico, por id (es un catálogo, no un dato de nadie).
pub async fn academic_sorts<C: ConnectionTrait>(db: &C) -> Result<HashMap<String, i32>, DbErr> {
    let mut m = HashMap::new();
    for f in db
        .query_all(stmt(
            "SELECT id::text, sort_order FROM academic_levels",
            vec![],
        ))
        .await?
    {
        m.insert(col(&f, 0)?, col(&f, 1)?);
    }
    Ok(m)
}

/// Lo que piden las ofertas dadas, en **una** consulta (sin N+1). Una oferta que no existe no
/// aparece en el mapa.
pub async fn requirements<C: ConnectionTrait>(
    db: &C,
    offers: &[String],
) -> Result<HashMap<String, Requirements>, DbErr> {
    let mut m: HashMap<String, Requirements> = HashMap::new();
    if offers.is_empty() {
        return Ok(m);
    }
    let filas = db
        .query_all(stmt(
            "SELECT o.id::text, o.is_experience_required, o.required_experience, al.sort_order, \
                (SELECT COALESCE(jsonb_agg(jsonb_build_object('id', s.id, 'code', s.code, \
                        'name', s.name, 'required', r.is_required, 'min', r.min_level::text) \
                        ORDER BY r.is_required DESC, s.code, s.id), '[]'::jsonb) \
                 FROM job_offer_skills r JOIN skills s ON s.id = r.skill_id \
                 WHERE r.job_offer_id = o.id)::text, \
                (SELECT COALESCE(jsonb_agg(jsonb_build_object('id', l.id, 'code', l.code, \
                        'name', l.name, 'min', r.min_proficiency::text) \
                        ORDER BY l.code, l.id), '[]'::jsonb) \
                 FROM job_offer_languages r JOIN languages l ON l.id = r.language_id \
                 WHERE r.job_offer_id = o.id)::text \
             FROM job_offers o \
             LEFT JOIN academic_levels al ON al.id = o.minimum_academic_level_id \
             WHERE o.id = ANY (string_to_array($1, ',')::uuid[])",
            vec![offers.join(",").into()],
        ))
        .await?;
    let texto = |v: &Json, k: &str| v.get(k).and_then(Json::as_str).map(str::to_string);
    let cat = |v: &Json| -> Option<Cat> {
        Some(Cat {
            id: texto(v, "id")?,
            code: texto(v, "code")?,
            name: texto(v, "name")?,
        })
    };
    for f in &filas {
        let lista = |i: usize| -> Result<Vec<Json>, DbErr> {
            let t: String = col(f, i)?;
            serde_json::from_str(&t).map_err(|_| DbErr::Custom("json ilegible".into()))
        };
        let exige: bool = col(f, 1)?;
        let anios: Option<i32> = col(f, 2)?;
        m.insert(
            col(f, 0)?,
            Requirements {
                skills: lista(4)?
                    .iter()
                    .filter_map(|v| {
                        Some(ReqSkill {
                            skill: cat(v)?,
                            required: v.get("required")?.as_bool()?,
                            min_level: texto(v, "min").as_deref().and_then(Level::parse),
                        })
                    })
                    .collect(),
                languages: lista(5)?
                    .iter()
                    .filter_map(|v| {
                        Some(ReqLanguage {
                            language: cat(v)?,
                            min_proficiency: texto(v, "min")
                                .as_deref()
                                .and_then(Proficiency::parse),
                        })
                    })
                    .collect(),
                required_years: anios.filter(|a| exige && *a > 0).map(|a| a as u32),
                academic_min: col(f, 3)?,
            },
        );
    }
    Ok(m)
}

/// El puntaje de **un** perfil contra varias ofertas: `{id → {score, breakdown}}`, y el puntaje
/// suelto para ordenar.
pub struct Scored {
    pub score: u8,
    pub json: Json,
}

pub fn score_all(
    profile: &Profile,
    reqs: &HashMap<String, Requirements>,
    today: Ymd,
) -> HashMap<String, Scored> {
    reqs.iter()
        .map(|(id, r)| {
            let m = compute(profile, r, today);
            (
                id.clone(),
                Scored {
                    score: m.score,
                    json: m.to_json(),
                },
            )
        })
        .collect()
}

/// El puntaje de una aplicación visto por el **reclutador**: se calcula únicamente con lo que
/// `applications::shared_view` devolvió (los campos consentidos), nunca con el perfil entero.
/// Lo que la persona no compartió no aplica. `None` si la oferta ya no existe.
pub async fn for_shared_view<C: ConnectionTrait>(
    db: &C,
    offer_id: &str,
    view: &Json,
) -> Result<Option<Json>, DbErr> {
    let Some(req) = requirements(db, &[offer_id.to_string()])
        .await?
        .remove(offer_id)
    else {
        return Ok(None);
    };
    let niveles = academic_sorts(db).await?;
    let perfil = Profile::from_shared(view, |id| niveles.get(id).copied());
    Ok(Some(compute(&perfil, &req, today(db).await?).to_json()))
}

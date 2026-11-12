//! SQL de las aplicaciones. Todo valor viaja como parámetro; los nombres de tabla que se
//! interpolan son siempre constantes de este archivo.
//!
//! Toda consulta de la persona filtra por **su** cuenta (`applicants.user_id`): una aplicación
//! ajena y una inexistente son indistinguibles (regla 8).

use sea_orm::{ConnectionTrait, DbBackend, DbErr, QueryResult, Statement, Value};
use serde_json::{json, Map, Value as Json};

use super::{availability, Availability, Status};
use crate::company_gate::VISIBLE_COMPANY_SQL;
use crate::crypto::PiiKeys;

pub(super) fn stmt(sql: &str, valores: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, valores)
}

pub(super) fn col<T: sea_orm::TryGetable>(r: &QueryResult, i: usize) -> Result<T, DbErr> {
    r.try_get_by_index(i)
}

pub(super) fn iso(c: &str) -> String {
    format!("to_char({c} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')")
}

fn una<T: sea_orm::TryGetable>(f: Option<QueryResult>) -> Result<T, DbErr> {
    f.ok_or_else(|| DbErr::Custom("sin fila".into()))?
        .try_get_by_index(0)
}

// ------------------------------------------------------------------ aplicar

/// Serializa lo que una misma persona hace con sus aplicaciones (duplicado y tope diario)
/// hasta el fin de la transacción.
pub async fn lock_applicant<C: ConnectionTrait>(db: &C, user_id: &str) -> Result<(), DbErr> {
    db.execute(stmt(
        "SELECT pg_advisory_xact_lock(hashtextextended('apply:' || $1::text, 0))",
        vec![user_id.to_string().into()],
    ))
    .await?;
    Ok(())
}

/// El `applicants.id` de la persona, si lo tiene.
pub async fn applicant_of<C: ConnectionTrait>(
    db: &C,
    user_id: &str,
) -> Result<Option<String>, DbErr> {
    db.query_one(stmt(
        "SELECT id::text FROM applicants WHERE user_id = $1::uuid",
        vec![user_id.to_string().into()],
    ))
    .await?
    .map(|f| col(&f, 0))
    .transpose()
}

/// La empresa de una oferta **visible**: publicada, de una empresa activa, verificada y con
/// membresía vigente. Cualquier otra cosa (incluida una inexistente) es `None`. Deja la oferta
/// bloqueada en modo compartido: no se edita ni se cierra mientras se aplica.
pub async fn visible_offer_company<C: ConnectionTrait>(
    db: &C,
    offer_id: &str,
) -> Result<Option<String>, DbErr> {
    // La misma regla que el listado de candidatos (`routes::offers_public`): publicada, no
    // vencida por `hiring_date` y de una empresa visible.
    let sql = format!(
        "SELECT o.company_id::text FROM job_offers o JOIN companies c ON c.id = o.company_id \
         WHERE o.id = $1::uuid AND o.status = 'published' AND o.publication_date IS NOT NULL \
           AND (o.hiring_date IS NULL OR o.hiring_date >= (now() AT TIME ZONE 'UTC')::date) \
           AND {VISIBLE_COMPANY_SQL} \
         FOR SHARE OF o"
    );
    db.query_one(stmt(&sql, vec![offer_id.to_string().into()]))
        .await?
        .map(|f| col(&f, 0))
        .transpose()
}

/// ¿La persona es reclutadora vigente de esa empresa?
pub async fn is_member<C: ConnectionTrait>(
    db: &C,
    user_id: &str,
    company_id: &str,
) -> Result<bool, DbErr> {
    let n: i64 = una(db
        .query_one(stmt(
            "SELECT count(*) FROM company_recruiters \
             WHERE user_id = $1::uuid AND company_id = $2::uuid AND deleted_at IS NULL",
            vec![user_id.to_string().into(), company_id.to_string().into()],
        ))
        .await?)?;
    Ok(n > 0)
}

/// La aplicación previa de la persona a esa oferta (retirada o no).
pub async fn existing<C: ConnectionTrait>(
    db: &C,
    applicant_id: &str,
    offer_id: &str,
) -> Result<Option<String>, DbErr> {
    db.query_one(stmt(
        "SELECT id::text FROM job_applications \
         WHERE applicant_id = $1::uuid AND job_offer_id = $2::uuid",
        vec![applicant_id.to_string().into(), offer_id.to_string().into()],
    ))
    .await?
    .map(|f| col(&f, 0))
    .transpose()
}

/// Aplicaciones creadas en las últimas 24 horas (cuentan las retiradas) y, si ya están en el
/// tope, los segundos hasta que baje de él.
pub async fn last_day<C: ConnectionTrait>(
    db: &C,
    applicant_id: &str,
    tope: i64,
) -> Result<(i64, i64), DbErr> {
    let n: i64 = una(db
        .query_one(stmt(
            "SELECT count(*) FROM job_applications \
             WHERE applicant_id = $1::uuid AND applied_at > now() - interval '24 hours'",
            vec![applicant_id.to_string().into()],
        ))
        .await?)?;
    if n < tope {
        return Ok((n, 0));
    }
    // Cuando salga de la ventana la que está en el lugar `tope` desde la más reciente, habrá cupo.
    let espera: i64 = una(db
        .query_one(stmt(
            "SELECT COALESCE(GREATEST(1, CEIL(EXTRACT(EPOCH FROM \
                    (applied_at + interval '24 hours' - now()))))::bigint, 1) \
             FROM job_applications \
             WHERE applicant_id = $1::uuid AND applied_at > now() - interval '24 hours' \
             ORDER BY applied_at DESC OFFSET $2 LIMIT 1",
            vec![applicant_id.to_string().into(), (tope - 1).into()],
        ))
        .await?)
    .unwrap_or(1);
    Ok((n, espera))
}

/// Crea la aplicación y su evento `null → applied` (`changed_by` = la persona).
pub async fn insert<C: ConnectionTrait>(
    db: &C,
    applicant_id: &str,
    user_id: &str,
    offer_id: &str,
    shared_fields: &[&str],
) -> Result<String, DbErr> {
    let id = migration::new_id().to_string();
    db.execute(stmt(
        "INSERT INTO job_applications (id, applicant_id, job_offer_id, status, shared_fields) \
         VALUES ($1::uuid, $2::uuid, $3::uuid, 'applied', $4::text::jsonb)",
        vec![
            id.clone().into(),
            applicant_id.to_string().into(),
            offer_id.to_string().into(),
            json!(shared_fields).to_string().into(),
        ],
    ))
    .await?;
    db.execute(stmt(
        "INSERT INTO job_application_events (id, job_application_id, from_status, to_status, changed_by) \
         VALUES ($1::uuid, $2::uuid, NULL, 'applied', $3::uuid)",
        vec![
            migration::new_id().to_string().into(),
            id.clone().into(),
            user_id.to_string().into(),
        ],
    ))
    .await?;
    Ok(id)
}

// ------------------------------------------------------------------ leer (la persona)

#[derive(Debug, Clone)]
pub struct Cat {
    pub id: String,
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct AppRow {
    pub id: String,
    pub status: String,
    pub applied_at: String,
    pub applied_micros: i64,
    pub status_changed_at: String,
    pub consented_at: String,
    pub shared_fields: Vec<String>,
    pub offer_id: String,
    pub offer_title: String,
    pub offer_status: String,
    pub confidential: bool,
    /// Solo si la oferta no es confidencial o ya se reveló a **esta** aplicación.
    pub company: Option<(String, String, Option<String>)>,
    pub state: Option<Cat>,
    pub city: Option<Cat>,
}

fn cat(f: &QueryResult, i: usize) -> Result<Option<Cat>, DbErr> {
    Ok(match col::<Option<String>>(f, i)? {
        Some(id) => Some(Cat {
            id,
            code: col(f, i + 1)?,
            name: col(f, i + 2)?,
        }),
        None => None,
    })
}

/// Las aplicaciones **de esta persona**, más nuevas primero, con un `id` opcional y filtro por
/// estado. Pide `limite + 1` filas para saber si hay más.
pub async fn list_mine<C: ConnectionTrait>(
    db: &C,
    user_id: &str,
    id: Option<&str>,
    status: Option<&str>,
    cursor: Option<(i64, String)>,
    limite: i64,
) -> Result<Vec<AppRow>, DbErr> {
    let (micros, cid) = match cursor {
        Some((m, i)) => (Some(m), Some(i)),
        None => (None, None),
    };
    let sql = format!(
        "SELECT a.id::text, a.status::text, {applied}, {applied_micros}, {changed}, {consented}, \
                a.shared_fields::text, o.id::text, o.name, o.status::text, o.is_confidential, \
                (NOT o.is_confidential OR a.company_revealed_at IS NOT NULL), \
                c.id::text, c.name, c.website, \
                st.id::text, st.code, st.name, ci.id::text, ci.code, ci.name \
         FROM job_applications a \
         JOIN applicants ap ON ap.id = a.applicant_id AND ap.user_id = $1::uuid \
         JOIN job_offers o ON o.id = a.job_offer_id \
         JOIN companies c ON c.id = o.company_id \
         LEFT JOIN states st ON st.id = o.state_id \
         LEFT JOIN cities ci ON ci.id = o.city_id \
         WHERE ($2::text IS NULL OR a.id = $2::uuid) \
           AND ($3::text IS NULL OR a.status::text = $3) \
           AND ($4::bigint IS NULL OR (a.applied_at, a.id) < \
                (timestamptz 'epoch' + $4::bigint * interval '1 microsecond', $5::uuid)) \
         ORDER BY a.applied_at DESC, a.id DESC LIMIT $6",
        applied = iso("a.applied_at"),
        applied_micros = "(EXTRACT(EPOCH FROM a.applied_at) * 1000000)::bigint",
        changed = iso("a.status_changed_at"),
        consented = iso("a.consented_at"),
    );
    db.query_all(stmt(
        &sql,
        vec![
            user_id.to_string().into(),
            id.map(str::to_string).into(),
            status.map(str::to_string).into(),
            micros.into(),
            cid.into(),
            (limite + 1).into(),
        ],
    ))
    .await?
    .iter()
    .map(|f| {
        let revelada: bool = col(f, 11)?;
        let campos: String = col(f, 6)?;
        let shared_fields: Vec<String> = serde_json::from_str(&campos)
            .map_err(|_| DbErr::Custom("shared_fields no es una lista".into()))?;
        Ok(AppRow {
            id: col(f, 0)?,
            status: col(f, 1)?,
            applied_at: col(f, 2)?,
            applied_micros: col(f, 3)?,
            status_changed_at: col(f, 4)?,
            consented_at: col(f, 5)?,
            shared_fields,
            offer_id: col(f, 7)?,
            offer_title: col(f, 8)?,
            offer_status: col(f, 9)?,
            confidential: col(f, 10)?,
            company: if revelada {
                Some((col(f, 12)?, col(f, 13)?, col(f, 14)?))
            } else {
                None
            },
            state: cat(f, 15)?,
            city: cat(f, 18)?,
        })
    })
    .collect()
}

/// `[(status, at)]` en orden cronológico: lo único del historial que ve la persona.
pub async fn history_of<C: ConnectionTrait>(
    db: &C,
    application_id: &str,
) -> Result<Vec<(String, String)>, DbErr> {
    db.query_all(stmt(
        &format!(
            "SELECT to_status::text, {} FROM job_application_events \
             WHERE job_application_id = $1::uuid ORDER BY created_at, id",
            iso("created_at")
        ),
        vec![application_id.to_string().into()],
    ))
    .await?
    .iter()
    .map(|f| Ok((col(f, 0)?, col(f, 1)?)))
    .collect()
}

// ------------------------------------------------------------------ lo compartido

struct Anchor {
    status: Status,
    applicant_id: Option<String>,
    user_id: Option<String>,
    shared: Vec<String>,
    closed_micros: Option<i64>,
}

async fn anchor<C: ConnectionTrait>(db: &C, id: &str) -> Result<Option<Anchor>, DbErr> {
    let Some(f) = db
        .query_one(stmt(
            "SELECT a.status::text, a.applicant_id::text, ap.user_id::text, a.shared_fields::text, \
                    (EXTRACT(EPOCH FROM o.closed_at) * 1000000)::bigint \
             FROM job_applications a \
             JOIN job_offers o ON o.id = a.job_offer_id \
             LEFT JOIN applicants ap ON ap.id = a.applicant_id \
             WHERE a.id = $1::uuid",
            vec![id.to_string().into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let estado: String = col(&f, 0)?;
    let campos: String = col(&f, 3)?;
    Ok(Some(Anchor {
        status: Status::parse(&estado).ok_or_else(|| DbErr::Custom("estado desconocido".into()))?,
        applicant_id: col(&f, 1)?,
        user_id: col(&f, 2)?,
        shared: serde_json::from_str(&campos)
            .map_err(|_| DbErr::Custom("shared_fields no es una lista".into()))?,
        closed_micros: col(&f, 4)?,
    }))
}

fn cat_json(f: &QueryResult, i: usize) -> Result<Json, DbErr> {
    Ok(match cat(f, i)? {
        Some(c) => json!({"id": c.id, "code": c.code, "name": c.name}),
        None => Json::Null,
    })
}

/// Lo que el panel puede ver de una aplicación: **solo** los campos consentidos del perfil
/// actual, o `None` si no está disponible (retirada, cuenta borrada o retención vencida).
///
/// Es la **única** función que lee datos de la persona para otra: cada campo sale de un brazo
/// del `match` y solo si su nombre está en `shared_fields`. Lo que no se marcó no se consulta
/// ni se descifra siquiera. El reloj entra como parámetro.
pub async fn shared_view<C: ConnectionTrait>(
    db: &C,
    pii: &PiiKeys,
    application_id: &str,
    now_micros: i64,
) -> Result<Option<Json>, DbErr> {
    let Some(id) = crate::offers::input::uuid_canonico(application_id) else {
        return Ok(None);
    };
    let Some(a) = anchor(db, &id).await? else {
        return Ok(None);
    };
    if availability(
        a.status,
        a.applicant_id.is_some(),
        a.closed_micros,
        now_micros,
    ) != Availability::Available
    {
        return Ok(None);
    }
    let (Some(applicant), Some(user)) = (a.applicant_id, a.user_id) else {
        return Ok(None);
    };
    let marcado = |n: &str| a.shared.iter().any(|f| f == n);
    // Nunca el dato ni el motivo: solo que falló.
    let cripto = |_: crate::crypto::CryptoError| DbErr::Custom("descifrado fallido".into());

    let mut v = Map::new();
    let persona = db
        .query_one(stmt(
            "SELECT first_name, last_name, email FROM users WHERE id = $1::uuid AND deleted_at IS NULL",
            vec![user.clone().into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("sin persona".into()))?;

    for campo in super::input::FIELDS {
        if !marcado(campo) {
            continue;
        }
        match campo {
            "display_name" => {
                let nombre: String = col(&persona, 0)?;
                let apellido: String = col(&persona, 1)?;
                v.insert(campo.into(), json!(super::display_name(&nombre, &apellido)));
            }
            "skills" => {
                let filas = db
                    .query_all(stmt(
                        "SELECT s.id::text, s.code, s.name, k.level::text, k.years_of_experience \
                         FROM applicant_skills k JOIN skills s ON s.id = k.skill_id \
                         WHERE k.applicant_id = $1::uuid ORDER BY s.code",
                        vec![applicant.clone().into()],
                    ))
                    .await?;
                let mut lista = Vec::new();
                for f in &filas {
                    lista.push(json!({
                        "skill": {"id": col::<String>(f, 0)?, "code": col::<String>(f, 1)?, "name": col::<String>(f, 2)?},
                        "level": col::<Option<String>>(f, 3)?,
                        "years_of_experience": col::<Option<i32>>(f, 4)?,
                    }));
                }
                v.insert(campo.into(), Json::Array(lista));
            }
            "experience" => {
                let con_descripcion = marcado("experience_descriptions");
                let filas = db
                    .query_all(stmt(
                        "SELECT company, title, start_date::text, end_date::text, description \
                         FROM applicant_experiences WHERE applicant_id = $1::uuid \
                         ORDER BY position, start_date DESC, id",
                        vec![applicant.clone().into()],
                    ))
                    .await?;
                let mut lista = Vec::new();
                for f in &filas {
                    let mut e = Map::new();
                    e.insert("company".into(), json!(col::<String>(f, 0)?));
                    e.insert("title".into(), json!(col::<String>(f, 1)?));
                    e.insert("start_date".into(), json!(col::<String>(f, 2)?));
                    e.insert("end_date".into(), json!(col::<Option<String>>(f, 3)?));
                    if con_descripcion {
                        let d = match col::<Option<Vec<u8>>>(f, 4)? {
                            Some(b) => Some(pii.decrypt(&b).map_err(cripto)?),
                            None => None,
                        };
                        e.insert("description".into(), json!(d));
                    }
                    lista.push(Json::Object(e));
                }
                v.insert(campo.into(), Json::Array(lista));
            }
            // La descripción de cada experiencia viaja dentro de `experience`.
            "experience_descriptions" => {}
            "languages" => {
                let filas = db
                    .query_all(stmt(
                        "SELECT l.id::text, l.code, l.name, al.proficiency::text \
                         FROM applicant_languages al JOIN languages l ON l.id = al.language_id \
                         WHERE al.applicant_id = $1::uuid ORDER BY l.code",
                        vec![applicant.clone().into()],
                    ))
                    .await?;
                let mut lista = Vec::new();
                for f in &filas {
                    lista.push(json!({
                        "language": {"id": col::<String>(f, 0)?, "code": col::<String>(f, 1)?, "name": col::<String>(f, 2)?},
                        "proficiency": col::<String>(f, 3)?,
                    }));
                }
                v.insert(campo.into(), Json::Array(lista));
            }
            "education" => {
                let f = db
                    .query_one(stmt(
                        "SELECT al.id::text, al.code, al.name, p.id::text, p.code, p.name, \
                                h.id::text, h.code, h.name \
                         FROM professional_data d \
                         LEFT JOIN academic_levels al ON al.id = d.last_academic_level_id \
                         LEFT JOIN professions p ON p.id = d.profession_id \
                         LEFT JOIN job_hierarchies h ON h.id = d.last_job_hierarchy_id \
                         WHERE d.applicant_id = $1::uuid",
                        vec![applicant.clone().into()],
                    ))
                    .await?;
                let e = match f {
                    Some(f) => json!({
                        "academic_level": cat_json(&f, 0)?,
                        "profession": cat_json(&f, 3)?,
                        "job_hierarchy": cat_json(&f, 6)?,
                    }),
                    None => {
                        json!({"academic_level": null, "profession": null, "job_hierarchy": null})
                    }
                };
                v.insert(campo.into(), e);
            }
            "profile_description" => {
                let d: Option<String> = match db
                    .query_one(stmt(
                        "SELECT description FROM professional_data WHERE applicant_id = $1::uuid",
                        vec![applicant.clone().into()],
                    ))
                    .await?
                {
                    Some(f) => col(&f, 0)?,
                    None => None,
                };
                v.insert(campo.into(), json!(d));
            }
            "email" => {
                let b: Vec<u8> = col(&persona, 2)?;
                v.insert(campo.into(), json!(pii.decrypt(&b).map_err(cripto)?));
            }
            "phone" => {
                let b: Option<Vec<u8>> = match db
                    .query_one(stmt(
                        "SELECT phone FROM personal_data WHERE applicant_id = $1::uuid",
                        vec![applicant.clone().into()],
                    ))
                    .await?
                {
                    Some(f) => col(&f, 0)?,
                    None => None,
                };
                let t = match b {
                    Some(b) => Some(pii.decrypt(&b).map_err(cripto)?),
                    None => None,
                };
                v.insert(campo.into(), json!(t));
            }
            _ => {}
        }
    }
    Ok(Some(Json::Object(v)))
}

//! SQL de las ofertas. Todo texto del usuario viaja como parámetro (`$n`), nunca interpolado;
//! los nombres de tabla que se interpolan son siempre constantes de este archivo.
//!
//! Cada consulta que toca una oferta por id filtra **también** por `company_id`: es la regla de
//! «si el listado filtra por dueño, la consulta por id filtra igual».

use sea_orm::{ConnectionTrait, DbBackend, DbErr, QueryResult, Statement, Value};

use super::input::{LanguageReq, Offer, SkillReq};

fn stmt(sql: &str, valores: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, valores)
}

/// `timestamptz` → ISO 8601 en UTC con `Z`.
fn ts(col: &str) -> String {
    format!("to_char({col} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')")
}

/// Lo que la base guarda de una oferta, más lo que calcula el servidor.
#[derive(Debug, Clone)]
pub struct Stored {
    pub id: String,
    pub status: String,
    pub publication_date: Option<String>,
    pub closed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub applications_count: i32,
    pub created_by: Option<(String, String)>,
    pub offer: Offer,
}

fn col<T: sea_orm::TryGetable>(r: &QueryResult, i: usize) -> Result<T, DbErr> {
    r.try_get_by_index(i)
}

/// Lee una oferta de **esta** empresa con sus requisitos. Con `bloquear`, deja la fila bloqueada
/// hasta el final de la transacción (`FOR UPDATE`): serializa las transiciones y las ediciones.
pub async fn load<C: ConnectionTrait>(
    db: &C,
    company_id: &str,
    id: &str,
    bloquear: bool,
) -> Result<Option<Stored>, DbErr> {
    let sql = format!(
        "SELECT o.id::text, o.status::text, o.publication_date::text, {closed}, {created}, {updated}, \
                o.applications_count, o.name, o.description, o.is_confidential, \
                o.is_experience_required, o.required_experience, o.main_language_id::text, \
                o.schedule, o.required_availability_to_travel, o.vacancies, o.hiring_date::text, \
                o.profession_id::text, o.employment_sector_id::text, o.job_hierarchy_id::text, \
                o.minimum_academic_level_id::text, o.state_id::text, o.city_id::text, \
                o.salary_range_id::text, o.working_shift_id::text, o.contract_type_id::text, \
                cr.id::text, u.first_name, u.last_name \
         FROM job_offers o \
         LEFT JOIN company_recruiters cr ON cr.id = o.created_by \
         LEFT JOIN users u ON u.id = cr.user_id AND u.deleted_at IS NULL \
         WHERE o.id = $1::uuid AND o.company_id = $2::uuid{lock}",
        closed = ts("o.closed_at"),
        created = ts("o.created_at"),
        updated = ts("o.updated_at"),
        lock = if bloquear { " FOR UPDATE OF o" } else { "" },
    );
    let Some(f) = db
        .query_one(stmt(
            &sql,
            vec![id.to_string().into(), company_id.to_string().into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let created_by = match (
        col::<Option<String>>(&f, 26)?,
        col::<Option<String>>(&f, 27)?,
        col::<Option<String>>(&f, 28)?,
    ) {
        (Some(rid), Some(nombre), Some(apellido)) => {
            let inicial: String = apellido.chars().take(1).collect();
            Some((rid, format!("{nombre} {inicial}.")))
        }
        _ => None,
    };

    let skills = db
        .query_all(stmt(
            "SELECT skill_id::text, is_required, min_level::text FROM job_offer_skills \
             WHERE job_offer_id = $1::uuid ORDER BY skill_id",
            vec![id.to_string().into()],
        ))
        .await?
        .iter()
        .map(|r| {
            Ok(SkillReq {
                skill_id: col(r, 0)?,
                is_required: col(r, 1)?,
                min_level: col(r, 2)?,
            })
        })
        .collect::<Result<Vec<_>, DbErr>>()?;
    let languages = db
        .query_all(stmt(
            "SELECT language_id::text, min_proficiency::text FROM job_offer_languages \
             WHERE job_offer_id = $1::uuid ORDER BY language_id",
            vec![id.to_string().into()],
        ))
        .await?
        .iter()
        .map(|r| {
            Ok(LanguageReq {
                language_id: col(r, 0)?,
                min_proficiency: col(r, 1)?,
            })
        })
        .collect::<Result<Vec<_>, DbErr>>()?;
    let driver_license_ids = db
        .query_all(stmt(
            "SELECT driver_license_id::text FROM job_offer_driver_licenses \
             WHERE job_offer_id = $1::uuid ORDER BY driver_license_id",
            vec![id.to_string().into()],
        ))
        .await?
        .iter()
        .map(|r| col(r, 0))
        .collect::<Result<Vec<String>, DbErr>>()?;

    Ok(Some(Stored {
        id: col(&f, 0)?,
        status: col(&f, 1)?,
        publication_date: col(&f, 2)?,
        closed_at: col(&f, 3)?,
        created_at: col(&f, 4)?,
        updated_at: col(&f, 5)?,
        applications_count: col(&f, 6)?,
        created_by,
        offer: Offer {
            title: col(&f, 7)?,
            description: col(&f, 8)?,
            is_confidential: col(&f, 9)?,
            is_experience_required: col(&f, 10)?,
            required_experience: col(&f, 11)?,
            main_language_id: col(&f, 12)?,
            schedule: col(&f, 13)?,
            required_availability_to_travel: col(&f, 14)?,
            vacancies: col(&f, 15)?,
            hiring_date: col(&f, 16)?,
            profession_id: col(&f, 17)?,
            employment_sector_id: col(&f, 18)?,
            job_hierarchy_id: col(&f, 19)?,
            minimum_academic_level_id: col(&f, 20)?,
            state_id: col(&f, 21)?,
            city_id: col(&f, 22)?,
            salary_range_id: col(&f, 23)?,
            working_shift_id: col(&f, 24)?,
            contract_type_id: col(&f, 25)?,
            skills,
            languages,
            driver_license_ids,
        },
    }))
}

fn valores_de(o: &Offer) -> Vec<Value> {
    vec![
        o.title.clone().into(),
        o.description.clone().into(),
        o.is_experience_required.into(),
        o.required_experience.into(),
        o.main_language_id.clone().into(),
        o.is_confidential.into(),
        o.schedule.clone().into(),
        o.required_availability_to_travel.into(),
        (!o.driver_license_ids.is_empty()).into(),
        o.vacancies.into(),
        o.hiring_date.clone().into(),
        o.profession_id.clone().into(),
        o.employment_sector_id.clone().into(),
        o.job_hierarchy_id.clone().into(),
        o.minimum_academic_level_id.clone().into(),
        o.state_id.clone().into(),
        o.city_id.clone().into(),
        o.salary_range_id.clone().into(),
        o.working_shift_id.clone().into(),
        o.contract_type_id.clone().into(),
    ]
}

/// Inserta la oferta (siempre en `draft`) y sus requisitos. Devuelve su id.
pub async fn insert<C: ConnectionTrait>(
    db: &C,
    company_id: &str,
    recruiter_id: &str,
    o: &Offer,
) -> Result<String, DbErr> {
    let id = migration::new_id().to_string();
    let mut v: Vec<Value> = vec![
        id.clone().into(),
        company_id.to_string().into(),
        recruiter_id.to_string().into(),
    ];
    v.extend(valores_de(o));
    db.execute(stmt(
        "INSERT INTO job_offers (id, company_id, created_by, name, description, \
            is_experience_required, required_experience, main_language_id, is_confidential, \
            schedule, required_availability_to_travel, required_driver_license, vacancies, \
            hiring_date, profession_id, employment_sector_id, job_hierarchy_id, \
            minimum_academic_level_id, state_id, city_id, salary_range_id, working_shift_id, \
            contract_type_id) \
         VALUES ($1::uuid, $2::uuid, $3::uuid, $4, $5, $6, $7, $8::uuid, $9, $10, $11, $12, $13, \
                 $14::date, $15::uuid, $16::uuid, $17::uuid, $18::uuid, $19::uuid, $20::uuid, \
                 $21::uuid, $22::uuid, $23::uuid)",
        v,
    ))
    .await?;
    insert_requirements(db, &id, o).await?;
    Ok(id)
}

/// Reemplaza todos los campos editables y los requisitos de la oferta.
pub async fn replace<C: ConnectionTrait>(
    db: &C,
    company_id: &str,
    id: &str,
    o: &Offer,
) -> Result<(), DbErr> {
    let mut v: Vec<Value> = vec![id.to_string().into(), company_id.to_string().into()];
    v.extend(valores_de(o));
    db.execute(stmt(
        "UPDATE job_offers SET name = $3, description = $4, is_experience_required = $5, \
            required_experience = $6, main_language_id = $7::uuid, is_confidential = $8, \
            schedule = $9, required_availability_to_travel = $10, required_driver_license = $11, \
            vacancies = $12, hiring_date = $13::date, profession_id = $14::uuid, \
            employment_sector_id = $15::uuid, job_hierarchy_id = $16::uuid, \
            minimum_academic_level_id = $17::uuid, state_id = $18::uuid, city_id = $19::uuid, \
            salary_range_id = $20::uuid, working_shift_id = $21::uuid, \
            contract_type_id = $22::uuid, updated_at = now() \
         WHERE id = $1::uuid AND company_id = $2::uuid",
        v,
    ))
    .await?;
    for tabla in REQ_TABLAS {
        db.execute(stmt(
            &format!("DELETE FROM {tabla} WHERE job_offer_id = $1::uuid"),
            vec![id.to_string().into()],
        ))
        .await?;
    }
    insert_requirements(db, id, o).await
}

const REQ_TABLAS: [&str; 3] = [
    "job_offer_skills",
    "job_offer_languages",
    "job_offer_driver_licenses",
];

async fn insert_requirements<C: ConnectionTrait>(db: &C, id: &str, o: &Offer) -> Result<(), DbErr> {
    for s in &o.skills {
        db.execute(stmt(
            "INSERT INTO job_offer_skills (job_offer_id, skill_id, is_required, min_level) \
             VALUES ($1::uuid, $2::uuid, $3, $4::skill_level)",
            vec![
                id.to_string().into(),
                s.skill_id.clone().into(),
                s.is_required.into(),
                s.min_level.clone().into(),
            ],
        ))
        .await?;
    }
    for l in &o.languages {
        db.execute(stmt(
            "INSERT INTO job_offer_languages (job_offer_id, language_id, min_proficiency) \
             VALUES ($1::uuid, $2::uuid, $3::proficiency)",
            vec![
                id.to_string().into(),
                l.language_id.clone().into(),
                l.min_proficiency.clone().into(),
            ],
        ))
        .await?;
    }
    for d in &o.driver_license_ids {
        db.execute(stmt(
            "INSERT INTO job_offer_driver_licenses (job_offer_id, driver_license_id) \
             VALUES ($1::uuid, $2::uuid)",
            vec![id.to_string().into(), d.clone().into()],
        ))
        .await?;
    }
    Ok(())
}

/// Hoy (UTC) según la base, como `AAAA-MM-DD`.
pub async fn today<C: ConnectionTrait>(db: &C) -> Result<String, DbErr> {
    db.query_one(stmt(
        "SELECT (now() AT TIME ZONE 'UTC')::date::text",
        vec![],
    ))
    .await?
    .ok_or_else(|| DbErr::Custom("sin fila".into()))?
    .try_get_by_index(0)
}

/// Cambia el estado de una oferta que ya se leyó con `FOR UPDATE`. `closed_at` se fija solo al
/// cerrar; `publication_date` solo en la primera publicación.
pub async fn set_status<C: ConnectionTrait>(
    db: &C,
    company_id: &str,
    id: &str,
    nuevo: &str,
) -> Result<Option<String>, DbErr> {
    let fila = db
        .query_one(stmt(
            "UPDATE job_offers SET status = $3::job_offer_status, \
                publication_date = CASE WHEN $3 = 'published' \
                    THEN COALESCE(publication_date, (now() AT TIME ZONE 'UTC')::date) \
                    ELSE publication_date END, \
                closed_at = CASE WHEN $3 = 'closed' THEN now() ELSE closed_at END, \
                updated_at = now() \
             WHERE id = $1::uuid AND company_id = $2::uuid \
             RETURNING publication_date::text",
            vec![
                id.to_string().into(),
                company_id.to_string().into(),
                nuevo.to_string().into(),
            ],
        ))
        .await?;
    fila.map(|f| col(&f, 0)).transpose()
}

pub async fn delete<C: ConnectionTrait>(db: &C, company_id: &str, id: &str) -> Result<(), DbErr> {
    db.execute(stmt(
        "DELETE FROM job_offers WHERE id = $1::uuid AND company_id = $2::uuid",
        vec![id.to_string().into(), company_id.to_string().into()],
    ))
    .await?;
    Ok(())
}

/// Serializa las creaciones de la misma empresa hasta el fin de la transacción.
pub async fn lock_company_creates<C: ConnectionTrait>(
    db: &C,
    company_id: &str,
) -> Result<(), DbErr> {
    db.execute(stmt(
        "SELECT pg_advisory_xact_lock(hashtextextended('offer_create:' || $1::text, 0))",
        vec![company_id.to_string().into()],
    ))
    .await?;
    Ok(())
}

/// Ofertas creadas por la empresa en la última hora y, si ya están en el tope, los segundos
/// hasta que la más antigua salga de la ventana.
pub async fn creates_last_hour<C: ConnectionTrait>(
    db: &C,
    company_id: &str,
) -> Result<(i64, i64), DbErr> {
    let f = db
        .query_one(stmt(
            "SELECT count(*), \
                    COALESCE(GREATEST(1, CEIL(EXTRACT(EPOCH FROM \
                        (min(created_at) + interval '1 hour' - now()))))::bigint, 1) \
             FROM job_offers WHERE company_id = $1::uuid \
               AND created_at > now() - interval '1 hour'",
            vec![company_id.to_string().into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("sin fila".into()))?;
    Ok((col(&f, 0)?, col(&f, 1)?))
}

/// Una fila del listado.
pub struct Row {
    pub id: String,
    pub title: String,
    pub status: String,
    pub is_confidential: bool,
    pub vacancies: Option<i32>,
    pub publication_date: Option<String>,
    pub applications_count: i32,
    pub created_at: String,
    /// Para el cursor: microsegundos Unix de `created_at`.
    pub created_micros: i64,
}

/// Una página del listado de la empresa: más nuevas primero (`created_at`, `id`), con filtro
/// opcional por estado y cursor `(micros, id)`. Pide `limite + 1` filas para saber si hay más.
pub async fn list<C: ConnectionTrait>(
    db: &C,
    company_id: &str,
    status: Option<&str>,
    cursor: Option<(i64, String)>,
    limite: u64,
) -> Result<Vec<Row>, DbErr> {
    let (micros, cid) = match cursor {
        Some((m, i)) => (Some(m), Some(i)),
        None => (None, None),
    };
    let sql = format!(
        "SELECT id::text, name, status::text, is_confidential, vacancies, publication_date::text, \
                applications_count, {created}, (EXTRACT(EPOCH FROM created_at) * 1000000)::bigint \
         FROM job_offers \
         WHERE company_id = $1::uuid \
           AND ($2::text IS NULL OR status::text = $2) \
           AND ($3::bigint IS NULL OR (created_at, id) < \
                (timestamptz 'epoch' + $3::bigint * interval '1 microsecond', $4::uuid)) \
         ORDER BY created_at DESC, id DESC LIMIT $5",
        created = ts("created_at"),
    );
    db.query_all(stmt(
        &sql,
        vec![
            company_id.to_string().into(),
            status.map(str::to_string).into(),
            micros.into(),
            cid.into(),
            ((limite + 1) as i64).into(),
        ],
    ))
    .await?
    .iter()
    .map(|f| {
        Ok(Row {
            id: col(f, 0)?,
            title: col(f, 1)?,
            status: col(f, 2)?,
            is_confidential: col(f, 3)?,
            vacancies: col(f, 4)?,
            publication_date: col(f, 5)?,
            applications_count: col(f, 6)?,
            created_at: col(f, 7)?,
            created_micros: col(f, 8)?,
        })
    })
    .collect()
}

/// ¿Existen todos (y activos) los ids en la tabla de catálogo? `tabla` es siempre una constante.
pub async fn all_active<C: ConnectionTrait>(
    db: &C,
    tabla: &'static str,
    ids: &[String],
) -> Result<bool, DbErr> {
    if ids.is_empty() {
        return Ok(true);
    }
    let f = db
        .query_one(stmt(
            &format!("SELECT count(*) FROM {tabla} WHERE active AND id = ANY($1::text[]::uuid[])"),
            vec![ids.to_vec().into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("sin fila".into()))?;
    let n: i64 = col(&f, 0)?;
    Ok(n as usize == ids.len())
}

/// ¿La ciudad existe, está activa y es de ese estado?
pub async fn city_in_state<C: ConnectionTrait>(
    db: &C,
    city_id: &str,
    state_id: &str,
) -> Result<bool, DbErr> {
    let f = db
        .query_one(stmt(
            "SELECT count(*) FROM cities WHERE active AND id = $1::uuid AND state_id = $2::uuid",
            vec![city_id.to_string().into(), state_id.to_string().into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("sin fila".into()))?;
    Ok(col::<i64>(&f, 0)? == 1)
}

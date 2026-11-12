//! SQL del panel del reclutador (Fase 5a, paso 7). Contrato: `api.md`, «Panel del reclutador».
//!
//! Toda consulta filtra por la empresa de quien llama (`job_offers.company_id`): una aplicación
//! de otra empresa y una inexistente son indistinguibles (regla 8, SX6). Todo valor viaja como
//! parámetro; lo único que se interpola es el sentido del orden, que sale de un `enum`.
//!
//! Lo que el panel lee de la persona en la **lista** es solo el nombre visible, y solo si la
//! aplicación sigue disponible; el detalle lo arma `store::shared_view`.

use sea_orm::{ConnectionTrait, DbErr};

use super::store::{col, iso, stmt};
use super::{availability, display_name, Availability, Status};
use crate::crypto::PiiKeys;

/// Orden de la lista: siempre por fecha de aplicación, con el id de desempate (PA9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Recent,
    Oldest,
}

impl Order {
    /// `sort` de la consulta. Vacío es un valor ilegal de un parámetro permitido; cualquier otro
    /// criterio (nombre, edad, puntaje…) no existe en el panel: se trata como parámetro no
    /// permitido. A diferencia del listado de ofertas, aquí `match` no es un valor futuro.
    pub fn parse(s: Option<&str>) -> Result<Self, OrderError> {
        match s {
            None | Some("recent") => Ok(Order::Recent),
            Some("oldest") => Ok(Order::Oldest),
            Some("") => Err(OrderError::Invalid),
            Some(_) => Err(OrderError::NotAllowed),
        }
    }

    fn sql(self) -> (&'static str, &'static str) {
        match self {
            Order::Recent => ("<", "DESC"),
            Order::Oldest => (">", "ASC"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderError {
    Invalid,
    NotAllowed,
}

/// `favorite` de la consulta: solo `true` o `false`.
pub fn parse_favorite(s: &str) -> Option<bool> {
    match s {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Una aplicación vista desde el panel.
#[derive(Debug, Clone)]
pub struct PanelRow {
    pub id: String,
    pub status: Status,
    pub applied_at: String,
    pub applied_micros: i64,
    pub status_changed_at: String,
    pub is_favorite: bool,
    pub availability: Availability,
    /// Solo si está disponible y la persona compartió su nombre (obligatorio al aplicar).
    pub display_name: Option<String>,
    pub shared_fields: Vec<String>,
    pub offer_id: String,
    pub offer_title: String,
    /// Interno: para marcar favoritos. **Nunca** sale en una respuesta.
    pub applicant_id: Option<String>,
    /// El instante de la base con el que se calculó la disponibilidad (para `shared_view`).
    pub now_micros: i64,
}

/// Filtros de la lista. `offer` o `id` acotan; `company` siempre.
pub struct Query<'a> {
    pub company: &'a str,
    pub offer: Option<&'a str>,
    pub id: Option<&'a str>,
    pub status: Option<Status>,
    pub favorite: Option<bool>,
    pub cursor: Option<(i64, String)>,
    pub order: Order,
    /// Pide `limit + 1` filas para saber si hay más.
    pub limit: i64,
}

/// Las aplicaciones de las ofertas de la empresa, por fecha, con su disponibilidad calculada.
pub async fn list<C: ConnectionTrait>(db: &C, q: &Query<'_>) -> Result<Vec<PanelRow>, DbErr> {
    let (cmp, dir) = q.order.sql();
    let (micros, cid) = match &q.cursor {
        Some((m, i)) => (Some(*m), Some(i.clone())),
        None => (None, None),
    };
    // El nombre solo se lee si la persona sigue ahí, no retiró y lo compartió; la retención
    // (que depende del reloj) se decide en Rust con `availability`.
    let sql = format!(
        "SELECT a.id::text, a.status::text, {applied}, \
                (EXTRACT(EPOCH FROM a.applied_at) * 1000000)::bigint, {changed}, \
                a.shared_fields::text, a.applicant_id::text, \
                (EXTRACT(EPOCH FROM o.closed_at) * 1000000)::bigint, \
                (EXTRACT(EPOCH FROM now()) * 1000000)::bigint, \
                f.id IS NOT NULL, \
                CASE WHEN a.status <> 'withdrawn' AND a.shared_fields ? 'display_name' \
                     THEN u.first_name END, \
                CASE WHEN a.status <> 'withdrawn' AND a.shared_fields ? 'display_name' \
                     THEN u.last_name END, \
                o.id::text, o.name \
         FROM job_applications a \
         JOIN job_offers o ON o.id = a.job_offer_id AND o.company_id = $1::uuid \
         LEFT JOIN applicants ap ON ap.id = a.applicant_id \
         LEFT JOIN users u ON u.id = ap.user_id AND u.deleted_at IS NULL \
         LEFT JOIN company_favorite_applicants f \
                ON f.company_id = o.company_id AND f.applicant_id = a.applicant_id \
         WHERE ($2::text IS NULL OR a.job_offer_id = $2::uuid) \
           AND ($3::text IS NULL OR a.id = $3::uuid) \
           AND ($4::text IS NULL OR a.status::text = $4) \
           AND ($5::boolean IS NULL OR (f.id IS NOT NULL) = $5) \
           AND ($6::bigint IS NULL OR (a.applied_at, a.id) {cmp} \
                (timestamptz 'epoch' + $6::bigint * interval '1 microsecond', $7::uuid)) \
         ORDER BY a.applied_at {dir}, a.id {dir} LIMIT $8",
        applied = iso("a.applied_at"),
        changed = iso("a.status_changed_at"),
    );
    db.query_all(stmt(
        &sql,
        vec![
            q.company.to_string().into(),
            q.offer.map(str::to_string).into(),
            q.id.map(str::to_string).into(),
            q.status.map(|s| s.as_str().to_string()).into(),
            q.favorite.into(),
            micros.into(),
            cid.into(),
            q.limit.into(),
        ],
    ))
    .await?
    .iter()
    .map(|f| {
        let estado: String = col(f, 1)?;
        let status =
            Status::parse(&estado).ok_or_else(|| DbErr::Custom("estado desconocido".into()))?;
        let campos: String = col(f, 5)?;
        let shared_fields: Vec<String> = serde_json::from_str(&campos)
            .map_err(|_| DbErr::Custom("shared_fields no es una lista".into()))?;
        let applicant_id: Option<String> = col(f, 6)?;
        let now_micros: i64 = col(f, 8)?;
        let disponible = availability(
            status,
            applicant_id.is_some(),
            col::<Option<i64>>(f, 7)?,
            now_micros,
        );
        let nombre: Option<String> = col(f, 10)?;
        let apellido: Option<String> = col(f, 11)?;
        let display = match (disponible, nombre, apellido) {
            (Availability::Available, Some(n), Some(a)) => Some(display_name(&n, &a)),
            _ => None,
        };
        Ok(PanelRow {
            id: col(f, 0)?,
            status,
            applied_at: col(f, 2)?,
            applied_micros: col(f, 3)?,
            status_changed_at: col(f, 4)?,
            is_favorite: col(f, 9)?,
            availability: disponible,
            display_name: display,
            shared_fields,
            offer_id: col(f, 12)?,
            offer_title: col(f, 13)?,
            applicant_id,
            now_micros,
        })
    })
    .collect()
}

/// Una aplicación de la empresa, o `None` si es de otra, no existe o el id está mal formado.
pub async fn one<C: ConnectionTrait>(
    db: &C,
    company: &str,
    id: &str,
) -> Result<Option<PanelRow>, DbErr> {
    let Some(id) = crate::offers::input::uuid_canonico(id) else {
        return Ok(None);
    };
    let filas = list(
        db,
        &Query {
            company,
            offer: None,
            id: Some(&id),
            status: None,
            favorite: None,
            cursor: None,
            order: Order::Recent,
            limit: 1,
        },
    )
    .await?;
    Ok(filas.into_iter().next())
}

/// ¿La oferta es de la empresa? (Cualquier estado: el panel también es de ofertas cerradas.)
pub async fn offer_of_company<C: ConnectionTrait>(
    db: &C,
    company: &str,
    offer: &str,
) -> Result<bool, DbErr> {
    let Some(offer) = crate::offers::input::uuid_canonico(offer) else {
        return Ok(false);
    };
    Ok(db
        .query_one(stmt(
            "SELECT 1 FROM job_offers WHERE id = $1::uuid AND company_id = $2::uuid",
            vec![offer.into(), company.to_string().into()],
        ))
        .await?
        .is_some())
}

/// Un paso del historial tal como lo ve el panel: sin quién decidió (BO3), con la nota interna.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub from: Option<String>,
    pub to: String,
    pub at: String,
    pub note: Option<String>,
}

/// El historial completo de una aplicación, en orden, con las notas descifradas.
pub async fn history<C: ConnectionTrait>(
    db: &C,
    pii: &PiiKeys,
    application_id: &str,
) -> Result<Vec<Step>, DbErr> {
    let filas = db
        .query_all(stmt(
            &format!(
                "SELECT from_status::text, to_status::text, {}, note \
                 FROM job_application_events WHERE job_application_id = $1::uuid \
                 ORDER BY created_at, id",
                iso("created_at")
            ),
            vec![application_id.to_string().into()],
        ))
        .await?;
    let mut pasos = Vec::with_capacity(filas.len());
    for f in &filas {
        let note = match col::<Option<Vec<u8>>>(f, 3)? {
            // Nunca el dato ni el motivo: solo que falló.
            Some(b) => Some(
                pii.decrypt(&b)
                    .map_err(|_| DbErr::Custom("descifrado fallido".into()))?,
            ),
            None => None,
        };
        pasos.push(Step {
            from: col(f, 0)?,
            to: col(f, 1)?,
            at: col(f, 2)?,
            note,
        });
    }
    Ok(pasos)
}

/// Marca a la persona como favorita de la empresa. Idempotente.
pub async fn add_favorite<C: ConnectionTrait>(
    db: &C,
    company: &str,
    applicant: &str,
) -> Result<(), DbErr> {
    db.execute(stmt(
        "INSERT INTO company_favorite_applicants (id, company_id, applicant_id) \
         VALUES ($1::uuid, $2::uuid, $3::uuid) \
         ON CONFLICT ON CONSTRAINT company_favorite_applicants_company_applicant_key DO NOTHING",
        vec![
            migration::new_id().to_string().into(),
            company.to_string().into(),
            applicant.to_string().into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Quita el favorito. Idempotente.
pub async fn remove_favorite<C: ConnectionTrait>(
    db: &C,
    company: &str,
    applicant: &str,
) -> Result<(), DbErr> {
    db.execute(stmt(
        "DELETE FROM company_favorite_applicants \
         WHERE company_id = $1::uuid AND applicant_id = $2::uuid",
        vec![company.to_string().into(), applicant.to_string().into()],
    ))
    .await?;
    Ok(())
}

/// Detalles leídos por el reclutador en la última hora (SX5) y, si ya está en el tope, los
/// segundos hasta que la más vieja salga de la ventana. Se cuentan sobre `access_audit`.
pub async fn views_last_hour<C: ConnectionTrait>(
    db: &C,
    user_id: &str,
) -> Result<(i64, i64), DbErr> {
    let f = db
        .query_one(stmt(
            "SELECT count(*), \
                    COALESCE(GREATEST(1, CEIL(EXTRACT(EPOCH FROM \
                        (min(created_at) + interval '1 hour' - now()))))::bigint, 1) \
             FROM access_audit \
             WHERE actor_user_id = $1::uuid AND action = 'application.view' \
               AND created_at > now() - interval '1 hour'",
            vec![user_id.to_string().into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("sin fila".into()))?;
    Ok((col(&f, 0)?, col(&f, 1)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_orden_es_por_fecha_y_nada_mas() {
        assert_eq!(Order::parse(None), Ok(Order::Recent));
        assert_eq!(Order::parse(Some("recent")), Ok(Order::Recent));
        assert_eq!(Order::parse(Some("oldest")), Ok(Order::Oldest));
        assert_eq!(Order::parse(Some("")), Err(OrderError::Invalid));
        for criterio in ["name", "age", "gender", "Recent", "score", "photo", "match"] {
            assert_eq!(
                Order::parse(Some(criterio)),
                Err(OrderError::NotAllowed),
                "{criterio}"
            );
        }
    }

    #[test]
    fn el_sentido_del_orden_va_con_su_comparacion() {
        assert_eq!(Order::Recent.sql(), ("<", "DESC"));
        assert_eq!(Order::Oldest.sql(), (">", "ASC"));
    }

    #[test]
    fn favorito_solo_true_o_false() {
        assert_eq!(parse_favorite("true"), Some(true));
        assert_eq!(parse_favorite("false"), Some(false));
        for malo in ["", "1", "si", "TRUE", "yes"] {
            assert_eq!(parse_favorite(malo), None, "{malo}");
        }
    }
}

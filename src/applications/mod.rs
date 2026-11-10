//! Aplicaciones a ofertas con consentimiento explícito (Fase 5a, paso 6).
//!
//! - `input`: el cuerpo de «aplicar» (consentimiento y `shared_fields`, conjunto cerrado).
//! - `external`: el registro privado de aplicaciones hechas fuera de la plataforma (HU-32).
//! - `store`: el SQL.
//! - Aquí: la máquina de estados, la disponibilidad (retención de 90 días) y `move_status`, la
//!   única función que cambia el estado de una aplicación (la persona al retirar; el panel del
//!   reclutador del paso 7 la usará tal cual).
//!
//! Contrato: `api.md`, «Aplicar, ver y retirar (candidato)».

pub mod external;
pub mod input;
pub mod store;

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, TransactionTrait};

use crate::crypto::PiiKeys;

/// Días que el reclutador sigue viendo lo compartido tras cerrarse la oferta (decisión 5).
pub const RETENTION_DAYS: i64 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Applied,
    UnderReview,
    Shortlisted,
    Rejected,
    Withdrawn,
    Hired,
}

impl Status {
    pub const ALL: [Status; 6] = [
        Status::Applied,
        Status::UnderReview,
        Status::Shortlisted,
        Status::Rejected,
        Status::Withdrawn,
        Status::Hired,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Applied => "applied",
            Status::UnderReview => "under_review",
            Status::Shortlisted => "shortlisted",
            Status::Rejected => "rejected",
            Status::Withdrawn => "withdrawn",
            Status::Hired => "hired",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == s)
    }
}

/// Quién mueve una aplicación.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Applicant,
    Recruiter,
}

/// La máquina de estados (api.md): ¿está permitido `from → to` para `actor`?
pub fn can_move(from: Status, to: Status, actor: Actor) -> bool {
    use Status::*;
    match actor {
        Actor::Recruiter => matches!(
            (from, to),
            (Applied, UnderReview)
                | (UnderReview, Shortlisted)
                | (UnderReview, Rejected)
                | (Shortlisted, Hired)
                | (Shortlisted, Rejected)
        ),
        Actor::Applicant => matches!(from, Applied | UnderReview | Shortlisted) && to == Withdrawn,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Available,
    Withdrawn,
    Deleted,
    RetentionExpired,
}

impl Availability {
    pub fn as_str(self) -> &'static str {
        match self {
            Availability::Available => "available",
            Availability::Withdrawn => "withdrawn",
            Availability::Deleted => "deleted",
            Availability::RetentionExpired => "retention_expired",
        }
    }
}

/// Qué puede ver el panel de una aplicación. El reloj entra como parámetro.
pub fn availability(
    status: Status,
    has_applicant: bool,
    offer_closed_at_micros: Option<i64>,
    now_micros: i64,
) -> Availability {
    const DIA_MICROS: i64 = 86_400 * 1_000_000;
    if !has_applicant {
        return Availability::Deleted;
    }
    if status == Status::Withdrawn {
        return Availability::Withdrawn;
    }
    match offer_closed_at_micros {
        Some(cierre) if now_micros >= cierre.saturating_add(RETENTION_DAYS * DIA_MICROS) => {
            Availability::RetentionExpired
        }
        _ => Availability::Available,
    }
}

/// De quién es la aplicación que se mueve. Toda consulta filtra por esto (regla 8).
#[derive(Debug, Clone, Copy)]
pub enum Scope<'a> {
    /// La persona dueña (`users.id`): solo puede retirar.
    Applicant { user_id: &'a str },
    /// El panel de una empresa (`companies.id`) y el reclutador que decide (`users.id`).
    Company {
        company_id: &'a str,
        user_id: &'a str,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct Moved {
    pub from: Status,
    pub to: Status,
    /// `false` solo al retirar una ya retirada (idempotente, sin segundo evento).
    pub changed: bool,
}

#[derive(Debug)]
pub enum MoveError {
    /// Ajena, inexistente o id mal formado.
    NotFound,
    /// La transición no está en la tabla; trae el estado vigente.
    Invalid(Status),
    Crypto,
    Db(DbErr),
}

/// Cambia el estado de una aplicación en **una transacción**: bloquea la fila, valida la
/// máquina, actualiza con `WHERE status = <de>`, escribe el evento y, al pasar a `shortlisted`
/// una oferta confidencial, fija `company_revealed_at`.
pub async fn move_status(
    db: &DatabaseConnection,
    pii: &PiiKeys,
    scope: Scope<'_>,
    application_id: &str,
    to: Status,
    note: Option<&str>,
) -> Result<Moved, MoveError> {
    let Some(id) = crate::offers::input::uuid_canonico(application_id) else {
        return Err(MoveError::NotFound);
    };
    // `filtro` une la aplicación con su dueño (la persona) o con su empresa; `$2` es ese id.
    let (actor, quien, filtro_id, filtro) = match scope {
        Scope::Applicant { user_id } => (
            Actor::Applicant,
            user_id,
            user_id,
            "JOIN applicants ap ON ap.id = a.applicant_id AND ap.user_id = $2::uuid",
        ),
        Scope::Company {
            company_id,
            user_id,
        } => (
            Actor::Recruiter,
            user_id,
            company_id,
            "JOIN companies c ON c.id = o.company_id AND c.id = $2::uuid",
        ),
    };
    // Los identificadores del alcance también se validan: nada mal formado llega al SQL.
    let Some(filtro_id) = crate::offers::input::uuid_canonico(filtro_id) else {
        return Err(MoveError::NotFound);
    };
    let Some(quien) = crate::offers::input::uuid_canonico(quien) else {
        return Err(MoveError::NotFound);
    };
    let nota = match note {
        Some(n) => Some(pii.encrypt(n).map_err(|_| MoveError::Crypto)?),
        None => None,
    };

    let txn = db.begin().await.map_err(MoveError::Db)?;
    let fila = txn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "SELECT a.status::text, o.is_confidential \
                 FROM job_applications a JOIN job_offers o ON o.id = a.job_offer_id {filtro} \
                 WHERE a.id = $1::uuid FOR UPDATE OF a"
            ),
            vec![id.clone().into(), filtro_id.into()],
        ))
        .await
        .map_err(MoveError::Db)?;
    let Some(fila) = fila else {
        return Err(MoveError::NotFound);
    };
    let actual: String = fila.try_get_by_index(0).map_err(MoveError::Db)?;
    let confidencial: bool = fila.try_get_by_index(1).map_err(MoveError::Db)?;
    let Some(de) = Status::parse(&actual) else {
        return Err(MoveError::Db(DbErr::Custom("estado desconocido".into())));
    };
    // Retirar una ya retirada es idempotente: sin segundo evento.
    if actor == Actor::Applicant && de == Status::Withdrawn && to == Status::Withdrawn {
        return Ok(Moved {
            from: de,
            to,
            changed: false,
        });
    }
    if !can_move(de, to, actor) {
        return Err(MoveError::Invalid(de));
    }
    let n = txn
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE job_applications SET status = $2::job_application_status, \
                status_changed_at = now(), \
                company_revealed_at = CASE WHEN $2 = 'shortlisted' AND $4 \
                    THEN COALESCE(company_revealed_at, now()) ELSE company_revealed_at END \
             WHERE id = $1::uuid AND status = $3::job_application_status",
            vec![
                id.clone().into(),
                to.as_str().into(),
                de.as_str().into(),
                confidencial.into(),
            ],
        ))
        .await
        .map_err(MoveError::Db)?
        .rows_affected();
    if n != 1 {
        return Err(MoveError::Invalid(de));
    }
    txn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO job_application_events \
            (id, job_application_id, from_status, to_status, changed_by, note) \
         VALUES ($1::uuid, $2::uuid, $3::job_application_status, $4::job_application_status, \
                 $5::uuid, $6)",
        vec![
            migration::new_id().to_string().into(),
            id.into(),
            de.as_str().into(),
            to.as_str().into(),
            quien.into(),
            nota.into(),
        ],
    ))
    .await
    .map_err(MoveError::Db)?;
    txn.commit().await.map_err(MoveError::Db)?;
    Ok(Moved {
        from: de,
        to,
        changed: true,
    })
}

pub use store::shared_view;

#[cfg(test)]
mod tests {
    use super::*;
    use Status::*;

    const DIA: i64 = 86_400 * 1_000_000;

    #[test]
    fn los_estados_van_y_vienen_como_texto() {
        let textos = [
            "applied",
            "under_review",
            "shortlisted",
            "rejected",
            "withdrawn",
            "hired",
        ];
        for (s, t) in Status::ALL.iter().zip(textos) {
            assert_eq!(s.as_str(), t);
            assert_eq!(Status::parse(t), Some(*s));
        }
        for malo in ["", "Applied", "closed", "selected", " applied", "hired "] {
            assert_eq!(Status::parse(malo), None, "{malo:?}");
        }
    }

    /// La tabla de transiciones del reclutador, completa: solo estas cinco existen.
    #[test]
    fn el_reclutador_solo_tiene_cinco_transiciones() {
        let permitidas = [
            (Applied, UnderReview),
            (UnderReview, Shortlisted),
            (UnderReview, Rejected),
            (Shortlisted, Hired),
            (Shortlisted, Rejected),
        ];
        for from in Status::ALL {
            for to in Status::ALL {
                assert_eq!(
                    can_move(from, to, Actor::Recruiter),
                    permitidas.contains(&(from, to)),
                    "{from:?} -> {to:?}"
                );
            }
        }
    }

    /// `hired` solo sale de `shortlisted` (la cadena del plan era ambigua; se resolvió así).
    #[test]
    fn hired_solo_desde_shortlisted() {
        for from in Status::ALL {
            assert_eq!(
                can_move(from, Hired, Actor::Recruiter),
                from == Shortlisted,
                "{from:?}"
            );
        }
    }

    /// `hired`, `rejected` y `withdrawn` son finales, y repetir un estado no es una transición.
    #[test]
    fn los_finales_no_se_reabren_ni_se_repite_un_estado() {
        for actor in [Actor::Applicant, Actor::Recruiter] {
            for fin in [Hired, Rejected, Withdrawn] {
                for to in Status::ALL {
                    assert!(!can_move(fin, to, actor), "{fin:?} -> {to:?} ({actor:?})");
                }
            }
            for s in Status::ALL {
                assert!(!can_move(s, s, actor), "{s:?} -> {s:?}");
            }
        }
    }

    /// La persona solo mueve a `withdrawn`, y solo desde un estado vivo.
    #[test]
    fn la_persona_solo_retira() {
        for from in Status::ALL {
            for to in Status::ALL {
                let vivo = matches!(from, Applied | UnderReview | Shortlisted);
                assert_eq!(
                    can_move(from, to, Actor::Applicant),
                    vivo && to == Withdrawn,
                    "{from:?} -> {to:?}"
                );
            }
        }
    }

    /// El reclutador no retira ni vuelve a `applied`.
    #[test]
    fn el_reclutador_no_retira_ni_reabre() {
        for from in Status::ALL {
            assert!(!can_move(from, Withdrawn, Actor::Recruiter));
            assert!(!can_move(from, Applied, Actor::Recruiter));
        }
    }

    #[test]
    fn disponibilidad_normal_y_sin_cierre() {
        assert_eq!(
            availability(Applied, true, None, 5 * DIA),
            Availability::Available
        );
        assert_eq!(
            availability(Hired, true, None, 500 * DIA),
            Availability::Available
        );
    }

    /// La retención corre desde `closed_at`: al día 90 exacto ya venció; un microsegundo antes, no.
    #[test]
    fn la_retencion_son_90_dias_desde_el_cierre() {
        let cierre = 1_000 * DIA;
        let limite = cierre + RETENTION_DAYS * DIA;
        assert_eq!(RETENTION_DAYS, 90);
        assert_eq!(
            availability(Shortlisted, true, Some(cierre), limite - 1),
            Availability::Available
        );
        assert_eq!(
            availability(Shortlisted, true, Some(cierre), limite),
            Availability::RetentionExpired
        );
        assert_eq!(
            availability(Shortlisted, true, Some(cierre), limite + 400 * DIA),
            Availability::RetentionExpired
        );
        // Una oferta aún no cerrada no vence nunca.
        assert_eq!(
            availability(Shortlisted, true, None, limite + 400 * DIA),
            Availability::Available
        );
    }

    /// Cuenta borrada gana a todo; retirada gana a la retención.
    #[test]
    fn la_disponibilidad_tiene_precedencia() {
        let cierre = 10 * DIA;
        let tarde = cierre + 100 * DIA;
        assert_eq!(
            availability(Withdrawn, false, Some(cierre), tarde),
            Availability::Deleted
        );
        assert_eq!(
            availability(Withdrawn, true, Some(cierre), tarde),
            Availability::Withdrawn
        );
        assert_eq!(
            availability(Withdrawn, true, None, tarde),
            Availability::Withdrawn
        );
        assert_eq!(availability(Applied, false, None, 0), Availability::Deleted);
        for (a, t) in [
            (Availability::Available, "available"),
            (Availability::Withdrawn, "withdrawn"),
            (Availability::Deleted, "deleted"),
            (Availability::RetentionExpired, "retention_expired"),
        ] {
            assert_eq!(a.as_str(), t);
        }
    }
}

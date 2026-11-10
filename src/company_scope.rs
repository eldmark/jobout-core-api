//! «De qué empresa es quien llama»: la empresa sale **siempre** de la fila de
//! `company_recruiters` de la persona autenticada, nunca del path ni del cuerpo.
//!
//! El 403 de la empresa (`empresa_no_verificada`, `membresia_vencida`) lo decide
//! `company_gate::check`, en cada petición.
//!
//! Una persona con el rol pero sin fila vigente en `company_recruiters` (la dieron de baja, o el
//! rol quedó suelto) no tiene empresa: `None`, y las rutas responden 404.

use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement};

/// La empresa de quien llama y su estado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanyCtx {
    pub company_id: String,
    /// `company_recruiters.id` de quien llama (queda en `job_offers.created_by`).
    pub recruiter_id: String,
}

/// La empresa de la persona `user_id` (UUID en texto), si pertenece a una.
pub async fn company_of<C: ConnectionTrait>(
    db: &C,
    user_id: &str,
) -> Result<Option<CompanyCtx>, DbErr> {
    let fila = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT c.id::text, cr.id::text \
             FROM company_recruiters cr \
             JOIN companies c ON c.id = cr.company_id \
             WHERE cr.user_id = $1::uuid AND cr.deleted_at IS NULL AND c.deleted_at IS NULL",
            vec![user_id.to_string().into()],
        ))
        .await?;
    fila.map(|f| {
        Ok(CompanyCtx {
            company_id: f.try_get_by_index(0)?,
            recruiter_id: f.try_get_by_index(1)?,
        })
    })
    .transpose()
}

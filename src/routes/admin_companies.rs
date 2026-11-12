//! Verificación de empresas y membresía por la plataforma (`company.verify`, solo el rol `admin`).
//! Contrato: `documentation/api.md`, «Verificación de la empresa y membresía».
//!
//! La verificación es manual (se revisa dominio y documento fuera de la API) y **nunca un
//! `UPDATE` a mano**: cada acción deja **una fila en `access_audit`** —sin el contenido, con la IP
//! hasheada— dentro de la misma transacción, así que si el rastro no se puede escribir la
//! operación falla (500). Repetir `verify` o `unverify` es idempotente y, al no cambiar nada, no
//! deja una segunda fila ni mueve `verified_at`.

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::StatusCode,
    routing::{get, post, put},
    Json,
};
use sea_orm::{ConnectionTrait, TransactionTrait};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    company_support::*,
    table::{Access, RouteSpec},
};
use crate::{auth_user::AuthUser, state::AppState};

const PERMISO: &str = "company.verify";
/// La membresía dura a lo sumo 400 días.
const MAX_DIAS_MEMBRESIA: i64 = 400;

pub fn routes() -> Vec<RouteSpec> {
    vec![
        RouteSpec {
            path: "/api/v1/admin/companies",
            methods: &["GET"],
            access: Access::Permission(PERMISO),
            body_limit: None,
            handler: get(list_companies),
        },
        RouteSpec {
            path: "/api/v1/admin/companies/{id}/verify",
            methods: &["POST"],
            access: Access::Permission(PERMISO),
            body_limit: None,
            handler: post(verify_company),
        },
        RouteSpec {
            path: "/api/v1/admin/companies/{id}/unverify",
            methods: &["POST"],
            access: Access::Permission(PERMISO),
            body_limit: None,
            handler: post(unverify_company),
        },
        RouteSpec {
            path: "/api/v1/admin/companies/{id}/membership",
            methods: &["PUT"],
            access: Access::Permission(PERMISO),
            body_limit: None,
            handler: put(set_membership),
        },
    ]
}

async fn list_companies(
    State(state): State<AppState>,
    user: AuthUser,
    ip: IpCliente,
    RawQuery(cruda): RawQuery,
) -> Result<Json<Value>, ApiError> {
    let consulta = parse_consulta(cruda.as_deref(), &["verified", "limit", "cursor"])?;
    let verificada = match consulta.get("verified").map(String::as_str) {
        None => None,
        Some("true") => Some(true),
        Some("false") => Some(false),
        Some(_) => return Err(ApiError::Invalid("El filtro verified no es válido.")),
    };
    let pagina = Pagina::de(&consulta)?;
    let mut valores: Vec<sea_orm::Value> = vec![];
    let mut filtros = String::new();
    if let Some(v) = verificada {
        valores.push(v.into());
        filtros.push_str(&format!(
            " AND (c.verified_at IS NOT NULL) = ${}",
            valores.len()
        ));
    }
    if let Some((m, id)) = &pagina.cursor {
        valores.push((*m).into());
        let t = desde_micros(valores.len());
        valores.push(id.clone().into());
        filtros.push_str(&format!(
            " AND (c.created_at, c.id) > ({t}, ${}::uuid)",
            valores.len()
        ));
    }

    let txn = state.db.begin().await.map_err(db("begin"))?;
    let filas = txn
        .query_all(stmt(
            &format!(
                "SELECT c.id::text, c.name, c.tax_id, c.website, {}, {}, u.email, \
                        (SELECT count(*) FROM company_recruiters cr \
                         WHERE cr.company_id = c.id AND cr.deleted_at IS NULL), {} \
                 FROM companies c LEFT JOIN users u ON u.id = c.created_by \
                 WHERE c.deleted_at IS NULL{filtros} \
                 ORDER BY c.created_at, c.id LIMIT {}",
                iso("c.created_at"),
                iso("c.verified_at"),
                micros("c.created_at"),
                pagina.limite + 1
            ),
            valores,
        ))
        .await
        .map_err(db("listar_empresas"))?;
    let mut items = vec![];
    let mut ultimo: Option<(i64, String)> = None;
    for (i, f) in filas.iter().enumerate() {
        if i as i64 == pagina.limite {
            break;
        }
        let texto = |n: usize| -> Result<Option<String>, ApiError> {
            f.try_get_by_index(n).map_err(db("leer_empresa_admin"))
        };
        let id = texto(0)?.unwrap_or_default();
        let nit_cifrado: Vec<u8> = f.try_get_by_index(2).map_err(db("leer_nit"))?;
        let nit = state
            .pii
            .decrypt(&nit_cifrado)
            .map_err(|_| interno("descifrar_nit"))?;
        // Solo lo que va tras la `@`: sirve para contrastarlo con el sitio, y nada más.
        let dominio = match f
            .try_get_by_index::<Option<Vec<u8>>>(6)
            .map_err(db("leer_correo"))?
        {
            None => None,
            Some(c) => {
                let correo = state
                    .pii
                    .decrypt(&c)
                    .map_err(|_| interno("descifrar_correo"))?;
                correo.rsplit_once('@').map(|(_, d)| d.to_string())
            }
        };
        let n: i64 = f.try_get_by_index(7).map_err(db("leer_conteo"))?;
        let m: i64 = f.try_get_by_index(8).map_err(db("leer_instante"))?;
        items.push(json!({
            "id": id,
            "name": texto(1)?,
            "tax_id": nit,
            "website": texto(3)?,
            "created_at": texto(4)?,
            "verified_at": texto(5)?,
            "creator_email_domain": dominio,
            "recruiters_count": n,
        }));
        ultimo = Some((m, id));
    }
    let siguiente = (filas.len() as i64 > pagina.limite)
        .then(|| ultimo.map(|(m, id)| codificar_cursor(m, &id)))
        .flatten();
    auditar(&txn, &state.pii, &user.id, "company.list", None, &ip).await?;
    txn.commit().await.map_err(db("commit"))?;
    Ok(Json(json!({"items": items, "next_cursor": siguiente})))
}

/// Bloquea la empresa (404 si no existe) y dice si quien llama es de ella.
async fn bloquear(txn: &impl ConnectionTrait, id: &str, actor: &str) -> Result<bool, ApiError> {
    let fila = txn
        .query_one(stmt(
            "SELECT EXISTS (SELECT 1 FROM company_recruiters cr \
                            WHERE cr.company_id = c.id AND cr.user_id = $2::uuid \
                              AND cr.deleted_at IS NULL) \
             FROM companies c WHERE c.id = $1::uuid AND c.deleted_at IS NULL FOR UPDATE OF c",
            vec![id.to_string().into(), actor.to_string().into()],
        ))
        .await
        .map_err(db("bloquear_empresa"))?
        .ok_or(ApiError::NotFound)?;
    fila.try_get_by_index(0).map_err(db("es_miembro"))
}

async fn verify_company(
    State(state): State<AppState>,
    user: AuthUser,
    ip: IpCliente,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    cambiar(&state, &user, &ip, &id, true).await
}

async fn unverify_company(
    State(state): State<AppState>,
    user: AuthUser,
    ip: IpCliente,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    cambiar(&state, &user, &ip, &id, false).await
}

/// `verify` y `unverify`: 404 → (solo `verify`) 403 si el admin es de esa empresa → cambio
/// condicional (`verified_at IS NULL` / `IS NOT NULL`) → una fila de auditoría **solo si cambió**.
async fn cambiar(
    state: &AppState,
    user: &AuthUser,
    ip: &IpCliente,
    id: &str,
    verificar: bool,
) -> Result<StatusCode, ApiError> {
    let id = id_de_ruta(id)?;
    let txn = state.db.begin().await.map_err(db("begin"))?;
    let es_miembro = bloquear(&txn, &id, &user.id).await?;
    if verificar && es_miembro {
        // Nadie verifica a su propia empresa.
        return Err(ApiError::NoPermission);
    }
    let sql = if verificar {
        "UPDATE companies SET verified_at = now(), updated_at = now(), updated_by = $2::uuid \
         WHERE id = $1::uuid AND verified_at IS NULL"
    } else {
        "UPDATE companies SET verified_at = NULL, updated_at = now(), updated_by = $2::uuid \
         WHERE id = $1::uuid AND verified_at IS NOT NULL"
    };
    let cambio = txn
        .execute(stmt(sql, vec![id.clone().into(), user.id.clone().into()]))
        .await
        .map_err(db("cambiar_verificacion"))?
        .rows_affected();
    if cambio == 1 {
        let accion = if verificar {
            "company.verify"
        } else {
            "company.unverify"
        };
        auditar(&txn, &state.pii, &user.id, accion, Some(&id), ip).await?;
    }
    txn.commit().await.map_err(db("commit"))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct MembershipIn {
    membership_type: Option<String>,
    start_date: Option<String>,
    expiration_date: Option<String>,
}

/// `PUT` reemplaza la membresía vigente (la anterior queda `cancelled`). Fechas `YYYY-MM-DD`,
/// vencimiento no anterior al inicio y a lo sumo 400 días después.
async fn set_membership(
    State(state): State<AppState>,
    user: AuthUser,
    ip: IpCliente,
    Path(id): Path<String>,
    cuerpo: Bytes,
) -> Result<StatusCode, ApiError> {
    let e: MembershipIn = parse_cerrado(
        &cuerpo,
        &["membership_type", "start_date", "expiration_date"],
    )?;
    let malo = || ApiError::Invalid("Los datos de la membresía no son válidos.");
    let (Some(tipo), Some(inicio), Some(fin)) =
        (e.membership_type, e.start_date, e.expiration_date)
    else {
        return Err(malo());
    };
    if super::profile::fecha(&inicio).is_none() || super::profile::fecha(&fin).is_none() {
        return Err(malo());
    }
    // Las dos fechas ya son calendario válido: el cast no puede fallar.
    let rango = state
        .db
        .query_one(stmt(
            "SELECT $2::date >= $1::date AND $2::date - $1::date <= $3::int, \
                    (SELECT count(*) FROM membership_types WHERE code = $4 AND active)",
            vec![
                inicio.clone().into(),
                fin.clone().into(),
                (MAX_DIAS_MEMBRESIA as i32).into(),
                tipo.clone().into(),
            ],
        ))
        .await
        .map_err(db("validar_membresia"))?
        .ok_or_else(|| interno("validar_membresia_fila"))?;
    let rango_ok: bool = rango.try_get_by_index(0).map_err(db("rango"))?;
    let tipo_ok: i64 = rango.try_get_by_index(1).map_err(db("tipo"))?;
    if !rango_ok || tipo_ok != 1 {
        return Err(malo());
    }

    let id = id_de_ruta(&id)?;
    let txn = state.db.begin().await.map_err(db("begin"))?;
    bloquear(&txn, &id, &user.id).await?;
    txn.execute(stmt(
        "UPDATE memberships SET status = 'cancelled', updated_at = now() \
         WHERE company_id = $1::uuid AND status = 'active'",
        vec![id.clone().into()],
    ))
    .await
    .map_err(db("cancelar_membresia"))?;
    txn.execute(stmt(
        "INSERT INTO memberships (id, company_id, membership_type_id, start_date, \
                                  expiration_date, status, created_by) \
         VALUES ($1::uuid, $2::uuid, (SELECT id FROM membership_types WHERE code = $3), \
                 $4::date, $5::date, 'active', $6::uuid)",
        vec![
            migration::new_id().to_string().into(),
            id.clone().into(),
            tipo.into(),
            inicio.into(),
            fin.into(),
            user.id.clone().into(),
        ],
    ))
    .await
    .map_err(db("crear_membresia"))?;
    auditar(
        &txn,
        &state.pii,
        &user.id,
        "company.membership_set",
        Some(&id),
        &ip,
    )
    .await?;
    txn.commit().await.map_err(db("commit"))?;
    Ok(StatusCode::NO_CONTENT)
}

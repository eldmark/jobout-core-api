//! Empresas, reclutadores e invitaciones (Fase 5a, paso 3). Contrato: `documentation/api.md`,
//! «Empresas, reclutadores e invitaciones».
//!
//! - **Una empresa es de quien pertenece a ella.** Las rutas `/company/...` toman la empresa de la
//!   membresía de quien llama (`company_recruiters`), nunca del path ni del cuerpo; un recurso de
//!   otra empresa, inexistente o con id mal formado es el mismo 404.
//! - **Una persona, una empresa**: el índice único parcial de `company_recruiters(user_id)` lo
//!   garantiza la base; el código lo comprueba antes para dar el 409 con su código.
//! - **Los roles de empresa solo se dan por dos caminos**: crear la empresa y aceptar una
//!   invitación. Ninguna ruta recibe un rol para asignarlo.
//! - **Invitaciones**: el token (256 bits) se devuelve una vez; en la base solo su SHA-256, y la
//!   invitación está atada al HMAC del correo del invitado. Aceptar es un único `UPDATE ... WHERE
//!   used_at IS NULL AND expires_at > now()`, así dos aceptaciones simultáneas dejan una.
//! - El NIT se guarda cifrado más un HMAC para la unicidad.
//!
//! Nada del NIT, del nombre de la empresa, de un correo ni de un token se escribe en un log, y los
//! errores son textos fijos.

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sea_orm::{ConnectionTrait, DatabaseTransaction, TransactionTrait};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{
    auth::{email_is_valid, normalize_email},
    company_support::*,
    table::{Access, RouteSpec},
};
use crate::{auth_user::AuthUser, state::AppState};

/// `POST`/`PUT` de la empresa: 16 KB (el global es 8 KB).
const BODY_EMPRESA: usize = 16 * 1024;
const MAX_EMPRESAS_DIA: i64 = 5;
const MAX_INVITACIONES_DIA: i64 = 20;
const MAX_INVITACIONES_PENDIENTES: i64 = 50;
const MAX_ACEPTACIONES_HORA: i64 = 10;
const DIA: i64 = 86_400;
const HORA: i64 = 3_600;
const KIND_CREAR: &str = "company_create";
const KIND_ACEPTAR: &str = "invitation_accept";

pub fn routes() -> Vec<RouteSpec> {
    vec![
        RouteSpec {
            path: "/api/v1/companies",
            methods: &["POST"],
            access: Access::Authenticated,
            body_limit: Some(BODY_EMPRESA),
            handler: post(create_company),
        },
        // `GET` es de cualquiera con empresa; `PUT` pide `company.update`: dos entradas de la
        // misma ruta, cada una con su acceso.
        RouteSpec {
            path: "/api/v1/company",
            methods: &["GET"],
            access: Access::Authenticated,
            body_limit: None,
            handler: get(get_company),
        },
        RouteSpec {
            path: "/api/v1/company",
            methods: &["PUT"],
            access: Access::Permission("company.update"),
            body_limit: Some(BODY_EMPRESA),
            handler: put(put_company),
        },
        RouteSpec {
            path: "/api/v1/company/recruiters",
            methods: &["GET"],
            access: Access::Permission("company.invite"),
            body_limit: None,
            handler: get(list_recruiters),
        },
        RouteSpec {
            path: "/api/v1/company/recruiters/{id}",
            methods: &["DELETE"],
            access: Access::Permission("company.invite"),
            body_limit: None,
            handler: delete(remove_recruiter),
        },
        RouteSpec {
            path: "/api/v1/company/invitations",
            methods: &["GET", "POST"],
            access: Access::Permission("company.invite"),
            body_limit: None,
            handler: get(list_invitations).post(create_invitation),
        },
        RouteSpec {
            path: "/api/v1/company/invitations/{id}",
            methods: &["DELETE"],
            access: Access::Permission("company.invite"),
            body_limit: None,
            handler: delete(revoke_invitation),
        },
        RouteSpec {
            path: "/api/v1/invitations/accept",
            methods: &["POST"],
            access: Access::Authenticated,
            body_limit: None,
            handler: post(accept_invitation),
        },
    ]
}

// ------------------------------------------------------------------ validación

#[derive(Deserialize)]
struct CompanyIn {
    name: Option<String>,
    tax_id: Option<String>,
    website: Option<String>,
    phone: Option<String>,
    description: Option<String>,
    country_id: Option<String>,
    employment_sector_id: Option<String>,
}

const CAMPOS_EMPRESA: [&str; 7] = [
    "name",
    "tax_id",
    "website",
    "phone",
    "description",
    "country_id",
    "employment_sector_id",
];

struct CompanyOk {
    name: String,
    /// Como lo escribió la persona, recortado: es lo que se devuelve y lo que se cifra.
    tax_id: String,
    /// Mayúsculas, sin guiones ni espacios: lo único que se compara.
    tax_id_norm: String,
    website: Option<String>,
    phone: Option<String>,
    description: Option<String>,
    country_id: String,
    sector_id: String,
}

fn validar_empresa(cuerpo: &Bytes) -> Result<CompanyOk, ApiError> {
    let e: CompanyIn = parse_cerrado(cuerpo, &CAMPOS_EMPRESA)?;
    let name = e.name.as_deref().map(str::trim).unwrap_or("");
    if !(2..=120).contains(&name.chars().count()) {
        return Err(ApiError::Invalid(
            "El nombre debe tener entre 2 y 120 caracteres.",
        ));
    }
    let tax_id = e.tax_id.as_deref().map(str::trim).unwrap_or("");
    let largo = tax_id.chars().count();
    let norm: String = tax_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if !(4..=20).contains(&largo)
        || !tax_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ' ')
        || norm.is_empty()
    {
        return Err(ApiError::Invalid(
            "El NIT debe tener entre 4 y 20 letras, dígitos, guiones o espacios.",
        ));
    }
    let website = texto_opcional(&e.website, 200, "El sitio web es demasiado largo.")?;
    if let Some(w) = &website {
        if !w.starts_with("https://")
            || w.len() <= "https://".len()
            || w.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(ApiError::Invalid("El sitio web debe empezar con https://."));
        }
    }
    let phone = texto_opcional(&e.phone, 20, "El teléfono es demasiado largo.")?;
    let description = texto_opcional(
        &e.description,
        2000,
        "La descripción es demasiado larga (máximo 2000 caracteres).",
    )?;
    let id = |v: &Option<String>| {
        v.as_deref()
            .and_then(super::profile::uuid_canonico)
            .ok_or(ApiError::Invalid("Un dato del catálogo no es válido."))
    };
    Ok(CompanyOk {
        name: name.to_string(),
        tax_id: tax_id.to_string(),
        tax_id_norm: norm,
        website,
        phone,
        description,
        country_id: id(&e.country_id)?,
        sector_id: id(&e.employment_sector_id)?,
    })
}

/// País y sector existen **y están activos**.
async fn validar_catalogos(conn: &impl ConnectionTrait, c: &CompanyOk) -> Result<(), ApiError> {
    let fila = conn
        .query_one(stmt(
            "SELECT (SELECT count(*) FROM countries WHERE active AND id = $1::uuid), \
                    (SELECT count(*) FROM employment_sectors WHERE active AND id = $2::uuid)",
            vec![c.country_id.clone().into(), c.sector_id.clone().into()],
        ))
        .await
        .map_err(db("validar_catalogos"))?
        .ok_or_else(|| interno("validar_catalogos_fila"))?;
    let pais: i64 = fila.try_get_by_index(0).map_err(db("catalogo_pais"))?;
    let sector: i64 = fila.try_get_by_index(1).map_err(db("catalogo_sector"))?;
    if pais != 1 || sector != 1 {
        return Err(ApiError::Invalid("Un dato del catálogo no existe."));
    }
    Ok(())
}

// ------------------------------------------------------------------ lectura

const HOY: &str = "(now() AT TIME ZONE 'UTC')::date";

/// La empresa tal como la ve alguien de ella.
async fn leer_empresa(
    state: &AppState,
    conn: &impl ConnectionTrait,
    company_id: &str,
    my_role: &str,
) -> Result<Value, ApiError> {
    let fila = conn
        .query_one(stmt(
            &format!(
                "SELECT c.id::text, c.name, c.tax_id, c.website, c.phone, c.description, \
                        c.country_id::text, c.employment_sector_id::text, \
                        c.verified_at IS NOT NULL, {}, {}, \
                        mt.code, m.start_date::text, m.expiration_date::text, \
                        (m.status = 'active' AND m.start_date <= {HOY} AND {HOY} <= m.expiration_date) \
                 FROM companies c \
                 LEFT JOIN LATERAL (SELECT * FROM memberships m WHERE m.company_id = c.id \
                                    ORDER BY m.created_at DESC, m.id DESC LIMIT 1) m ON true \
                 LEFT JOIN membership_types mt ON mt.id = m.membership_type_id \
                 WHERE c.id = $1::uuid AND c.deleted_at IS NULL",
                iso("c.verified_at"),
                iso("c.created_at"),
            ),
            vec![company_id.to_string().into()],
        ))
        .await
        .map_err(db("leer_empresa"))?
        .ok_or(ApiError::NotFound)?;
    let g = |i: usize| -> Result<Option<String>, ApiError> {
        fila.try_get_by_index(i).map_err(db("leer_empresa_col"))
    };
    let cifrado: Vec<u8> = fila.try_get_by_index(2).map_err(db("leer_nit"))?;
    let nit = state
        .pii
        .decrypt(&cifrado)
        .map_err(|_| interno("descifrar_nit"))?;
    let verificada: bool = fila.try_get_by_index(8).map_err(db("leer_verificada"))?;
    let membership = match g(11)? {
        None => Value::Null,
        Some(tipo) => {
            let vigente: Option<bool> = fila.try_get_by_index(14).map_err(db("leer_vigente"))?;
            json!({
                "type": tipo,
                "start_date": g(12)?,
                "expiration_date": g(13)?,
                "status": if vigente == Some(true) { "active" } else { "expired" },
            })
        }
    };
    Ok(json!({
        "id": g(0)?,
        "name": g(1)?,
        "tax_id": nit,
        "website": g(3)?,
        "phone": g(4)?,
        "description": g(5)?,
        "country_id": g(6)?,
        "employment_sector_id": g(7)?,
        "verified": verificada,
        "verified_at": g(9)?,
        "membership": membership,
        "my_role": my_role,
        "created_at": g(10)?,
    }))
}

async fn get_company(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let mia = mi_empresa(&state.db, &user.id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(
        leer_empresa(&state, &state.db, &mia.company_id, &mia.role).await?,
    ))
}

// ------------------------------------------------------------------ crear y editar

/// Crea la empresa **sin verificar**, la fila de reclutador y el rol `company_admin`, todo en una
/// transacción. Orden: cuerpo (422) → ya tiene empresa (409) → NIT repetido (409) → límite (429).
async fn create_company(
    State(state): State<AppState>,
    user: AuthUser,
    cuerpo: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let c = validar_empresa(&cuerpo)?;
    validar_catalogos(&state.db, &c).await?;
    let cifrado = state
        .pii
        .encrypt(&c.tax_id)
        .map_err(|_| interno("cifrar_nit"))?;
    let hash = state.pii.tax_id_hash(&c.tax_id_norm);

    let txn = state.db.begin().await.map_err(db("begin"))?;
    candado(
        &txn,
        &format!("company_create:{}", user.id),
        "candado_crear",
    )
    .await?;
    if mi_empresa(&txn, &user.id).await?.is_some() {
        return Err(YA_TIENE_EMPRESA);
    }
    if nit_usado(&txn, &hash, None).await? {
        return Err(duplicada());
    }
    if let Some(espera) =
        espera_por_intentos(&txn, KIND_CREAR, &user.id, DIA, MAX_EMPRESAS_DIA).await?
    {
        return Err(ApiError::TooMany(espera));
    }
    let id = migration::new_id().to_string();
    let insertada = txn
        .execute(stmt(
            "INSERT INTO companies (id, name, tax_id, tax_id_hash, website, phone, description, \
                                    country_id, employment_sector_id, created_by) \
             VALUES ($1::uuid, $2, $3, $4, $5, $6, $7, $8::uuid, $9::uuid, $10::uuid)",
            vec![
                id.clone().into(),
                c.name.into(),
                cifrado.into(),
                hash.into(),
                c.website.into(),
                c.phone.into(),
                c.description.into(),
                c.country_id.into(),
                c.sector_id.into(),
                user.id.clone().into(),
            ],
        ))
        .await;
    if let Err(e) = insertada {
        return Err(if viola_unico(&e, "tax_id_hash") {
            duplicada()
        } else {
            db("insertar_empresa")(e)
        });
    }
    unir(&txn, &user.id, &id, "company_admin").await?;
    registrar_intento(&txn, KIND_CREAR, &user.id).await?;
    txn.commit().await.map_err(db("commit"))?;

    let empresa = leer_empresa(&state, &state.db, &id, "company_admin").await?;
    Ok((StatusCode::CREATED, Json(empresa)))
}

fn duplicada() -> ApiError {
    ApiError::Conflict(
        "empresa_duplicada",
        "No se pudo registrar la empresa con esos datos.",
    )
}

/// ¿Otra empresa (distinta de `salvo`) ya usa este NIT?
async fn nit_usado(
    conn: &impl ConnectionTrait,
    hash: &[u8],
    salvo: Option<&str>,
) -> Result<bool, ApiError> {
    let fila = conn
        .query_one(stmt(
            "SELECT count(*) FROM companies \
             WHERE tax_id_hash = $1 AND ($2::uuid IS NULL OR id <> $2::uuid)",
            vec![hash.to_vec().into(), salvo.map(str::to_string).into()],
        ))
        .await
        .map_err(db("nit_usado"))?
        .ok_or_else(|| interno("nit_usado_fila"))?;
    let n: i64 = fila.try_get_by_index(0).map_err(db("nit_usado_n"))?;
    Ok(n > 0)
}

/// Pone a la persona en la empresa con ese rol: su fila de reclutador y el rol de la cuenta.
/// Los dos únicos caminos que llaman aquí son crear la empresa y aceptar una invitación.
async fn unir(
    txn: &DatabaseTransaction,
    user_id: &str,
    company_id: &str,
    rol: &str,
) -> Result<(), ApiError> {
    let r = txn
        .execute(stmt(
            "INSERT INTO company_recruiters (id, user_id, company_id, role, created_by) \
             VALUES ($1::uuid, $2::uuid, $3::uuid, $4, $2::uuid)",
            vec![
                migration::new_id().to_string().into(),
                user_id.to_string().into(),
                company_id.to_string().into(),
                rol.to_string().into(),
            ],
        ))
        .await;
    if let Err(e) = r {
        return Err(
            if viola_unico(&e, "company_recruiters_one_company_per_user") {
                YA_TIENE_EMPRESA
            } else {
                db("insertar_reclutador")(e)
            },
        );
    }
    txn.execute(stmt(
        "INSERT INTO user_roles (user_id, role_id) \
         SELECT $1::uuid, id FROM roles WHERE code = $2 AND deleted_at IS NULL \
         ON CONFLICT DO NOTHING",
        vec![user_id.to_string().into(), rol.to_string().into()],
    ))
    .await
    .map_err(db("dar_rol"))?;
    Ok(())
}

async fn put_company(
    State(state): State<AppState>,
    user: AuthUser,
    cuerpo: Bytes,
) -> Result<StatusCode, ApiError> {
    let c = validar_empresa(&cuerpo)?;
    validar_catalogos(&state.db, &c).await?;
    let mia = mi_empresa(&state.db, &user.id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let cifrado = state
        .pii
        .encrypt(&c.tax_id)
        .map_err(|_| interno("cifrar_nit"))?;
    let hash = state.pii.tax_id_hash(&c.tax_id_norm);

    let txn = state.db.begin().await.map_err(db("begin"))?;
    let actual = txn
        .query_one(stmt(
            "SELECT verified_at IS NOT NULL, tax_id_hash FROM companies \
             WHERE id = $1::uuid AND deleted_at IS NULL FOR UPDATE",
            vec![mia.company_id.clone().into()],
        ))
        .await
        .map_err(db("bloquear_empresa"))?
        .ok_or(ApiError::NotFound)?;
    let verificada: bool = actual.try_get_by_index(0).map_err(db("leer_verificada"))?;
    let hash_actual: Vec<u8> = actual.try_get_by_index(1).map_err(db("leer_hash"))?;
    if hash != hash_actual {
        if verificada {
            return Err(ApiError::Conflict(
                "nit_inmutable",
                "El NIT de una empresa verificada no se puede cambiar.",
            ));
        }
        if nit_usado(&txn, &hash, Some(&mia.company_id)).await? {
            return Err(duplicada());
        }
    }
    let r = txn
        .execute(stmt(
            "UPDATE companies SET name = $2, tax_id = $3, tax_id_hash = $4, website = $5, \
                    phone = $6, description = $7, country_id = $8::uuid, \
                    employment_sector_id = $9::uuid, updated_at = now(), updated_by = $10::uuid \
             WHERE id = $1::uuid",
            vec![
                mia.company_id.into(),
                c.name.into(),
                cifrado.into(),
                hash.into(),
                c.website.into(),
                c.phone.into(),
                c.description.into(),
                c.country_id.into(),
                c.sector_id.into(),
                user.id.into(),
            ],
        ))
        .await;
    if let Err(e) = r {
        return Err(if viola_unico(&e, "tax_id_hash") {
            duplicada()
        } else {
            db("actualizar_empresa")(e)
        });
    }
    txn.commit().await.map_err(db("commit"))?;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------ reclutadores

async fn list_recruiters(
    State(state): State<AppState>,
    user: AuthUser,
    RawQuery(cruda): RawQuery,
) -> Result<Json<Value>, ApiError> {
    let consulta = parse_consulta(cruda.as_deref(), &["limit", "cursor"])?;
    let pagina = Pagina::de(&consulta)?;
    let mia = mi_empresa(&state.db, &user.id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let mut valores: Vec<sea_orm::Value> = vec![mia.company_id.into()];
    let mut cursor_sql = String::new();
    if let Some((m, id)) = &pagina.cursor {
        cursor_sql = format!(
            "AND (cr.created_at, cr.id) > ({}, $3::uuid)",
            desde_micros(2)
        );
        valores.push((*m).into());
        valores.push(id.clone().into());
    }
    let filas = state
        .db
        .query_all(stmt(
            &format!(
                "SELECT cr.id::text, u.first_name, u.last_name, cr.position, cr.role, {}, {} \
                 FROM company_recruiters cr JOIN users u ON u.id = cr.user_id \
                 WHERE cr.company_id = $1::uuid AND cr.deleted_at IS NULL {cursor_sql} \
                 ORDER BY cr.created_at, cr.id LIMIT {}",
                iso("cr.created_at"),
                micros("cr.created_at"),
                pagina.limite + 1
            ),
            valores,
        ))
        .await
        .map_err(db("listar_reclutadores"))?;
    let mut items = vec![];
    let mut ultimo: Option<(i64, String)> = None;
    for (i, f) in filas.iter().enumerate() {
        if i as i64 == pagina.limite {
            break;
        }
        let g = |n: usize| -> Result<Option<String>, ApiError> {
            f.try_get_by_index(n).map_err(db("leer_reclutador"))
        };
        let id = g(0)?.unwrap_or_default();
        let m: i64 = f.try_get_by_index(6).map_err(db("leer_reclutador_t"))?;
        // Nombre y primer apellido: nunca correo, teléfono ni apellido de casada.
        items.push(json!({
            "id": id,
            "display_name": format!("{} {}", g(1)?.unwrap_or_default(), g(2)?.unwrap_or_default()),
            "position": g(3)?,
            "role": g(4)?,
            "joined_at": g(5)?,
        }));
        ultimo = Some((m, id));
    }
    let siguiente = (filas.len() as i64 > pagina.limite)
        .then(|| ultimo.map(|(m, id)| codificar_cursor(m, &id)))
        .flatten();
    Ok(Json(json!({"items": items, "next_cursor": siguiente})))
}

/// Quita a un reclutador de **mi** empresa (también uno mismo): borra su fila (`deleted_at`) y le
/// quita los roles de empresa. No toca la empresa, sus ofertas ni su cuenta de candidato. El único
/// `company_admin` no puede salir.
async fn remove_recruiter(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = id_de_ruta(&id)?;
    let mia = mi_empresa(&state.db, &user.id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let txn = state.db.begin().await.map_err(db("begin"))?;
    bloquear_empresa(&txn, &mia.company_id).await?;
    let objetivo = txn
        .query_one(stmt(
            "SELECT user_id::text, role FROM company_recruiters \
             WHERE id = $1::uuid AND company_id = $2::uuid AND deleted_at IS NULL",
            vec![id.clone().into(), mia.company_id.clone().into()],
        ))
        .await
        .map_err(db("leer_objetivo"))?
        .ok_or(ApiError::NotFound)?;
    let objetivo_user: String = objetivo.try_get_by_index(0).map_err(db("objetivo_user"))?;
    let objetivo_rol: String = objetivo.try_get_by_index(1).map_err(db("objetivo_rol"))?;
    if objetivo_rol == "company_admin"
        && otros_administradores(&txn, &mia.company_id, &objetivo_user).await? == 0
    {
        return Err(ULTIMO_ADMIN);
    }
    txn.execute(stmt(
        "UPDATE company_recruiters SET deleted_at = now(), updated_at = now(), \
                updated_by = $2::uuid WHERE id = $1::uuid",
        vec![id.into(), user.id.into()],
    ))
    .await
    .map_err(db("quitar_reclutador"))?;
    txn.execute(stmt(
        "DELETE FROM user_roles WHERE user_id = $1::uuid AND role_id IN \
         (SELECT id FROM roles WHERE code IN ('company_admin', 'recruiter'))",
        vec![objetivo_user.into()],
    ))
    .await
    .map_err(db("quitar_roles"))?;
    txn.commit().await.map_err(db("commit"))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Serializa a quienes cambian la plantilla de una empresa (quitar, salir, borrar la cuenta).
pub(crate) async fn bloquear_empresa(
    txn: &impl ConnectionTrait,
    company_id: &str,
) -> Result<(), ApiError> {
    txn.execute(stmt(
        "SELECT 1 FROM companies WHERE id = $1::uuid FOR UPDATE",
        vec![company_id.to_string().into()],
    ))
    .await
    .map_err(db("bloquear_empresa"))?;
    Ok(())
}

/// Administradores vigentes de la empresa que no son `user_id`.
pub(crate) async fn otros_administradores(
    txn: &impl ConnectionTrait,
    company_id: &str,
    user_id: &str,
) -> Result<i64, ApiError> {
    let f = txn
        .query_one(stmt(
            "SELECT count(*) FROM company_recruiters \
             WHERE company_id = $1::uuid AND role = 'company_admin' AND deleted_at IS NULL \
               AND user_id <> $2::uuid",
            vec![company_id.to_string().into(), user_id.to_string().into()],
        ))
        .await
        .map_err(db("contar_administradores"))?
        .ok_or_else(|| interno("contar_administradores_fila"))?;
    f.try_get_by_index(0)
        .map_err(db("contar_administradores_n"))
}

// ------------------------------------------------------------------ invitaciones

#[derive(Deserialize)]
struct InvitationIn {
    email: Option<String>,
    role: Option<String>,
}

const ESTADO_INVITACION: &str = "CASE WHEN i.used_at IS NOT NULL THEN 'used' \
     WHEN i.expires_at <= now() THEN 'expired' ELSE 'pending' END";

fn sha256(texto: &str) -> Vec<u8> {
    Sha256::digest(texto.as_bytes()).to_vec()
}

async fn create_invitation(
    State(state): State<AppState>,
    user: AuthUser,
    cuerpo: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let e: InvitationIn = parse_cerrado(&cuerpo, &["email", "role"])?;
    let email = normalize_email(e.email.as_deref().unwrap_or(""));
    if !email_is_valid(&email) {
        return Err(ApiError::Invalid("El correo no es válido."));
    }
    let rol = match e.role.as_deref() {
        Some(r @ ("recruiter" | "company_admin")) => r.to_string(),
        _ => {
            return Err(ApiError::Invalid(
                "El rol debe ser recruiter o company_admin.",
            ))
        }
    };
    let mia = mi_empresa(&state.db, &user.id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let email_hash = state.pii.email_hash(&email);

    use aes_gcm::aead::{rand_core::RngCore, OsRng};
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);

    let txn = state.db.begin().await.map_err(db("begin"))?;
    bloquear_empresa(&txn, &mia.company_id).await?;
    // Quien ya es de MI empresa lo ve cualquiera de ella; quien es de otra no se revela: la
    // invitación se emite igual y el 409 sale solo al aceptar.
    let ya = txn
        .query_one(stmt(
            "SELECT count(*) FROM company_recruiters cr JOIN users u ON u.id = cr.user_id \
             WHERE cr.company_id = $1::uuid AND cr.deleted_at IS NULL AND u.email_hash = $2",
            vec![mia.company_id.clone().into(), email_hash.clone().into()],
        ))
        .await
        .map_err(db("invitado_miembro"))?
        .ok_or_else(|| interno("invitado_miembro_fila"))?;
    let ya: i64 = ya.try_get_by_index(0).map_err(db("invitado_miembro_n"))?;
    if ya > 0 {
        return Err(YA_TIENE_EMPRESA);
    }
    let cuentas = txn
        .query_one(stmt(
            "SELECT count(*) FILTER (WHERE used_at IS NULL AND revoked_at IS NULL \
                                       AND expires_at > now()), \
                    count(*) FILTER (WHERE created_at > now() - interval '1 day'), \
                    COALESCE(ceil(extract(epoch from (min(created_at) \
                        FILTER (WHERE created_at > now() - interval '1 day') \
                        + interval '1 day' - now()))), 1)::bigint \
             FROM company_invitations WHERE company_id = $1::uuid",
            vec![mia.company_id.clone().into()],
        ))
        .await
        .map_err(db("contar_invitaciones"))?
        .ok_or_else(|| interno("contar_invitaciones_fila"))?;
    let pendientes: i64 = cuentas.try_get_by_index(0).map_err(db("pendientes"))?;
    let hoy: i64 = cuentas.try_get_by_index(1).map_err(db("del_dia"))?;
    let espera: i64 = cuentas.try_get_by_index(2).map_err(db("espera"))?;
    if pendientes >= MAX_INVITACIONES_PENDIENTES {
        return Err(ApiError::Invalid(
            "Hay demasiadas invitaciones pendientes. Revoca alguna antes de invitar.",
        ));
    }
    if hoy >= MAX_INVITACIONES_DIA {
        return Err(ApiError::TooMany(espera.max(1) as u64));
    }
    let id = migration::new_id().to_string();
    let fila = txn
        .query_one(stmt(
            &format!(
                "INSERT INTO company_invitations \
                    (id, company_id, email_hash, role, token_hash, expires_at, created_by) \
                 VALUES ($1::uuid, $2::uuid, $3, $4, $5, now() + interval '7 days', $6::uuid) \
                 RETURNING {}, {}",
                iso("expires_at"),
                iso("created_at"),
            ),
            vec![
                id.clone().into(),
                mia.company_id.into(),
                email_hash.into(),
                rol.clone().into(),
                sha256(&token).into(),
                user.id.into(),
            ],
        ))
        .await
        .map_err(db("insertar_invitacion"))?
        .ok_or_else(|| interno("insertar_invitacion_fila"))?;
    txn.commit().await.map_err(db("commit"))?;
    let expira: String = fila.try_get_by_index(0).map_err(db("leer_expira"))?;
    let creada: String = fila.try_get_by_index(1).map_err(db("leer_creada"))?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id, "role": rol, "status": "pending",
            "expires_at": expira, "created_at": creada,
            "token": token,
        })),
    ))
}

async fn list_invitations(
    State(state): State<AppState>,
    user: AuthUser,
    RawQuery(cruda): RawQuery,
) -> Result<Json<Value>, ApiError> {
    let consulta = parse_consulta(cruda.as_deref(), &["limit", "cursor"])?;
    let pagina = Pagina::de(&consulta)?;
    let mia = mi_empresa(&state.db, &user.id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let mut valores: Vec<sea_orm::Value> = vec![mia.company_id.into()];
    let mut cursor_sql = String::new();
    if let Some((m, id)) = &pagina.cursor {
        cursor_sql = format!("AND (i.created_at, i.id) < ({}, $3::uuid)", desde_micros(2));
        valores.push((*m).into());
        valores.push(id.clone().into());
    }
    let filas = state
        .db
        .query_all(stmt(
            &format!(
                "SELECT i.id::text, i.role, {ESTADO_INVITACION}, {}, {}, {} \
                 FROM company_invitations i \
                 WHERE i.company_id = $1::uuid AND i.revoked_at IS NULL {cursor_sql} \
                 ORDER BY i.created_at DESC, i.id DESC LIMIT {}",
                iso("i.expires_at"),
                iso("i.created_at"),
                micros("i.created_at"),
                pagina.limite + 1
            ),
            valores,
        ))
        .await
        .map_err(db("listar_invitaciones"))?;
    let mut items = vec![];
    let mut ultimo: Option<(i64, String)> = None;
    for (i, f) in filas.iter().enumerate() {
        if i as i64 == pagina.limite {
            break;
        }
        let g = |n: usize| -> Result<String, ApiError> {
            f.try_get_by_index(n).map_err(db("leer_invitacion"))
        };
        let id = g(0)?;
        let m: i64 = f.try_get_by_index(5).map_err(db("leer_invitacion_t"))?;
        items.push(json!({
            "id": id, "role": g(1)?, "status": g(2)?,
            "expires_at": g(3)?, "created_at": g(4)?,
        }));
        ultimo = Some((m, id));
    }
    let siguiente = (filas.len() as i64 > pagina.limite)
        .then(|| ultimo.map(|(m, id)| codificar_cursor(m, &id)))
        .flatten();
    Ok(Json(json!({"items": items, "next_cursor": siguiente})))
}

async fn revoke_invitation(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = id_de_ruta(&id)?;
    let mia = mi_empresa(&state.db, &user.id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let n = state
        .db
        .execute(stmt(
            "UPDATE company_invitations SET revoked_at = now() \
             WHERE id = $1::uuid AND company_id = $2::uuid \
               AND revoked_at IS NULL AND used_at IS NULL",
            vec![id.into(), mia.company_id.into()],
        ))
        .await
        .map_err(db("revocar_invitacion"))?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct AcceptIn {
    token: Option<String>,
}

/// Orden: cuerpo (422) → límite (429: 10 intentos por persona y hora, acierte o no) → token
/// desconocido, vencido, usado, revocado o de otro correo (404, la misma respuesta) → la persona
/// ya tiene empresa (409).
async fn accept_invitation(
    State(state): State<AppState>,
    user: AuthUser,
    cuerpo: Bytes,
) -> Result<Json<Value>, ApiError> {
    let e: AcceptIn = parse_cerrado(&cuerpo, &["token"])?;
    let token = match e.token.as_deref() {
        Some(t) if !t.is_empty() && t.chars().count() <= 200 => t.to_string(),
        _ => return Err(ApiError::Invalid("El token no es válido.")),
    };

    // El intento se anota en su propia transacción: cuenta aunque lo que sigue falle.
    {
        let txn = state.db.begin().await.map_err(db("begin"))?;
        candado(
            &txn,
            &format!("{KIND_ACEPTAR}:{}", user.id),
            "candado_aceptar",
        )
        .await?;
        if let Some(espera) =
            espera_por_intentos(&txn, KIND_ACEPTAR, &user.id, HORA, MAX_ACEPTACIONES_HORA).await?
        {
            return Err(ApiError::TooMany(espera));
        }
        registrar_intento(&txn, KIND_ACEPTAR, &user.id).await?;
        txn.commit().await.map_err(db("commit"))?;
    }

    let txn = state.db.begin().await.map_err(db("begin"))?;
    // Un solo uso: el UPDATE es la puerta. Solo la cuenta cuyo correo es el de la invitación
    // pasa, y una invitación de una empresa borrada tampoco.
    let usada = txn
        .query_one(stmt(
            "UPDATE company_invitations i SET used_at = now() \
             WHERE i.token_hash = $1 AND i.used_at IS NULL AND i.revoked_at IS NULL \
               AND i.expires_at > now() \
               AND i.email_hash = (SELECT u.email_hash FROM users u WHERE u.id = $2::uuid) \
               AND EXISTS (SELECT 1 FROM companies c WHERE c.id = i.company_id \
                           AND c.deleted_at IS NULL) \
             RETURNING i.company_id::text, i.role",
            vec![sha256(&token).into(), user.id.clone().into()],
        ))
        .await
        .map_err(db("gastar_invitacion"))?
        .ok_or(ApiError::InvalidInvitation)?;
    let company_id: String = usada.try_get_by_index(0).map_err(db("inv_empresa"))?;
    let rol: String = usada.try_get_by_index(1).map_err(db("inv_rol"))?;
    if mi_empresa(&txn, &user.id).await?.is_some() {
        // Al soltar la transacción la invitación vuelve a quedar sin usar.
        return Err(YA_TIENE_EMPRESA);
    }
    unir(&txn, &user.id, &company_id, &rol).await?;
    let nombre = txn
        .query_one(stmt(
            "SELECT name FROM companies WHERE id = $1::uuid",
            vec![company_id.clone().into()],
        ))
        .await
        .map_err(db("inv_nombre"))?
        .ok_or_else(|| interno("inv_nombre_fila"))?;
    let nombre: String = nombre.try_get_by_index(0).map_err(db("inv_nombre_col"))?;
    txn.commit().await.map_err(db("commit"))?;
    Ok(Json(json!({
        "company": {"id": company_id, "name": nombre},
        "role": rol,
    })))
}

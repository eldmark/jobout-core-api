//! Catálogos de las ofertas y las empresas (Fase 5, paso 1): la geografía encadenada
//! `countries -> states -> cities -> zones`, `salary_ranges`, `working_shifts`,
//! `contract_types`, `employment_sectors`, `driver_licenses` y `membership_types`.
//! Forma estándar de db.md. Solo se siembra Guatemala; `zones` queda vacía.

use sea_orm::{DbBackend, Statement};
use sea_orm_migration::prelude::*;
use uuid::Uuid;

use crate::{new_id, offer_seeds as seeds};

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Los diez, de padre a hijo; `down` los borra al revés.
const TABLAS: [&str; 10] = [
    "countries",
    "states",
    "cities",
    "zones",
    "salary_ranges",
    "working_shifts",
    "contract_types",
    "employment_sectors",
    "driver_licenses",
    "membership_types",
];

/// La forma estándar. `extra` son columnas propias (con su coma final) y `restricciones`
/// restricciones de tabla (con su coma inicial).
fn create(tabla: &str, extra: &str, restricciones: &str) -> String {
    format!(
        "CREATE TABLE {tabla} (
            id          uuid PRIMARY KEY,
            code        varchar NOT NULL UNIQUE,
            name        varchar NOT NULL,
            description text,
            active      boolean NOT NULL DEFAULT true,
            sort_order  integer NOT NULL DEFAULT 0,
            {extra}
            created_at  timestamptz NOT NULL DEFAULT now(),
            created_by  uuid,
            updated_at  timestamptz NOT NULL DEFAULT now(),
            updated_by  uuid
            {restricciones}
        );"
    )
}

/// Un eslabón de la cadena geográfica: FK al padre y nombre único dentro de él (el índice de
/// esa restricción también sirve para buscar los hijos de un padre).
fn geografia(tabla: &str, col: &str, padre: &str) -> String {
    create(
        tabla,
        &format!("{col} uuid NOT NULL REFERENCES {padre} (id),"),
        &format!(", UNIQUE ({col}, name)"),
    )
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(&create("countries", "", "")).await?;
        for (tabla, col, padre) in [
            ("states", "country_id", "countries"),
            ("cities", "state_id", "states"),
            ("zones", "city_id", "cities"),
        ] {
            db.execute_unprepared(&geografia(tabla, col, padre)).await?;
        }
        db.execute_unprepared(&create(
            "salary_ranges",
            "currency    char(3) NOT NULL DEFAULT 'GTQ',
            min_amount  numeric(12, 2) NOT NULL CHECK (min_amount >= 0),
            max_amount  numeric(12, 2),",
            ", CHECK (max_amount IS NULL OR max_amount >= min_amount)",
        ))
        .await?;
        for tabla in [
            "working_shifts",
            "contract_types",
            "employment_sectors",
            "driver_licenses",
            "membership_types",
        ] {
            db.execute_unprepared(&create(tabla, "", "")).await?;
        }

        // Guatemala: 22 departamentos y 340 municipios. `zones` no se siembra.
        let pais = new_id();
        fila(db, "countries", pais, "gt", "Guatemala", 0, None, None).await?;
        for (code, nombre, municipios) in seeds::GT_DEPARTAMENTOS {
            let estado = new_id();
            fila(
                db,
                "states",
                estado,
                code,
                nombre,
                0,
                Some(("country_id", pais)),
                None,
            )
            .await?;
            for municipio in *municipios {
                // El municipio de Guatemala es la capital de la República.
                let descripcion = (*code == "guatemala" && *municipio == "Guatemala")
                    .then_some("Ciudad de Guatemala, capital de la República.");
                fila(
                    db,
                    "cities",
                    new_id(),
                    &seeds::code_municipio(code, municipio),
                    municipio,
                    0,
                    Some(("state_id", estado)),
                    descripcion,
                )
                .await?;
            }
        }

        // Los rangos y las jornadas van de menor a mayor, en el orden de la lista.
        for (i, (code, nombre, min, max)) in seeds::SALARY_RANGES.iter().enumerate() {
            db.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO salary_ranges (id, code, name, sort_order, currency, min_amount, max_amount) \
                 VALUES ($1::text::uuid, $2, $3, $4, 'GTQ', $5::text::numeric, $6::text::numeric)",
                [
                    new_id().to_string().into(),
                    (*code).into(),
                    (*nombre).into(),
                    (i as i32).into(),
                    min.to_string().into(),
                    max.map(|m| m.to_string()).into(),
                ],
            ))
            .await?;
        }
        for (tabla, lista) in [
            ("working_shifts", seeds::WORKING_SHIFTS),
            ("contract_types", seeds::CONTRACT_TYPES),
            ("employment_sectors", seeds::EMPLOYMENT_SECTORS),
            ("driver_licenses", seeds::DRIVER_LICENSES),
            ("membership_types", seeds::MEMBERSHIP_TYPES),
        ] {
            for (i, (code, nombre)) in lista.iter().enumerate() {
                fila(db, tabla, new_id(), code, nombre, i as i32, None, None).await?;
            }
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for tabla in TABLAS.iter().rev() {
            db.execute_unprepared(&format!("DROP TABLE IF EXISTS {tabla};"))
                .await?;
        }
        Ok(())
    }
}

/// Inserta una fila estándar con parámetros enlazados (los nombres llevan tildes y comillas
/// potenciales, y así nada se interpola). `padre` es (columna, id) en la geografía.
#[allow(clippy::too_many_arguments)]
async fn fila(
    db: &impl ConnectionTrait,
    tabla: &str,
    id: Uuid,
    code: &str,
    name: &str,
    orden: i32,
    padre: Option<(&str, Uuid)>,
    descripcion: Option<&str>,
) -> Result<(), DbErr> {
    let (cols, marcas) = match padre {
        Some((col, _)) => (format!(", {col}"), ", $6::text::uuid"),
        None => (String::new(), ""),
    };
    let mut valores: Vec<sea_orm::Value> = vec![
        id.to_string().into(),
        code.into(),
        name.into(),
        orden.into(),
        descripcion.map(str::to_string).into(),
    ];
    if let Some((_, p)) = padre {
        valores.push(p.to_string().into());
    }
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "INSERT INTO {tabla} (id, code, name, sort_order, description{cols}) \
             VALUES ($1::text::uuid, $2, $3, $4, $5{marcas})"
        ),
        valores,
    ))
    .await
    .map(|_| ())
}

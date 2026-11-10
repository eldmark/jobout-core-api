//! Casos de verdad de los catálogos de ofertas (CT1–CT3 y CT5 de docs/working/fase5-ofertas-empresas.md).
//! La integridad de las semillas en Rust (22 departamentos, municipios sin repetir) es unitaria y
//! vive en `migration/src/offer_seeds.rs`. El endpoint está en `tests/offer_catalogs.rs`.

mod common;

use common::{error_de, esquema, TestDb};
use migration::{Migrator, MigratorTrait};
use sea_orm::ConnectionTrait;

const CATALOGOS: [&str; 10] = [
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

/// Municipios por departamento según la división oficial (340 en total). Es un oráculo escrito
/// aparte de la semilla: si alguien quita o agrega un municipio, esta tabla lo detecta.
const MUNICIPIOS: [(&str, i64); 22] = [
    ("alta-verapaz", 17),
    ("baja-verapaz", 8),
    ("chimaltenango", 16),
    ("chiquimula", 11),
    ("el-progreso", 8),
    ("escuintla", 14),
    ("guatemala", 17),
    ("huehuetenango", 33),
    ("izabal", 5),
    ("jalapa", 7),
    ("jutiapa", 17),
    ("peten", 14),
    ("quetzaltenango", 24),
    ("quiche", 21),
    ("retalhuleu", 9),
    ("sacatepequez", 16),
    ("san-marcos", 30),
    ("santa-rosa", 14),
    ("solola", 19),
    ("suchitepequez", 21),
    ("totonicapan", 8),
    ("zacapa", 11),
];

async fn columna(db: &TestDb, tabla: &str, col: &str) -> Option<(String, String)> {
    let r = db
        .conn
        .query_one(sea_orm::Statement::from_string(
            db.conn.get_database_backend(),
            format!(
                "SELECT data_type, is_nullable FROM information_schema.columns \
                 WHERE table_schema = 'public' AND table_name = '{tabla}' AND column_name = '{col}'"
            ),
        ))
        .await
        .unwrap()?;
    Some((
        r.try_get_by_index(0).unwrap(),
        r.try_get_by_index(1).unwrap(),
    ))
}

async fn existe(db: &TestDb, tabla: &str) -> i64 {
    db.count(&format!(
        "SELECT count(*) FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = '{tabla}'"
    ))
    .await
}

/// CT1: los diez catálogos existen con la forma estándar y `code` único.
#[tokio::test]
async fn ct1_los_catalogos_tienen_la_forma_estandar() {
    let db = TestDb::migrated().await;
    for t in CATALOGOS {
        for (col, tipo, nulo) in [
            ("id", "uuid", "NO"),
            ("code", "character varying", "NO"),
            ("name", "character varying", "NO"),
            ("description", "text", "YES"),
            ("active", "boolean", "NO"),
            ("sort_order", "integer", "NO"),
            ("created_at", "timestamp with time zone", "NO"),
            ("created_by", "uuid", "YES"),
            ("updated_at", "timestamp with time zone", "NO"),
            ("updated_by", "uuid", "YES"),
        ] {
            assert_eq!(
                columna(&db, t, col).await,
                Some((tipo.to_string(), nulo.to_string())),
                "{t}.{col}"
            );
        }
        let unicos = db
            .count(&format!(
                "SELECT count(*) FROM pg_indexes WHERE tablename = '{t}' \
                 AND indexdef LIKE 'CREATE UNIQUE INDEX%(code)%'"
            ))
            .await;
        assert_eq!(unicos, 1, "{t}.code debe ser único");
    }
    // Lo propio de cada uno.
    for (t, col, tipo, nulo) in [
        ("states", "country_id", "uuid", "NO"),
        ("cities", "state_id", "uuid", "NO"),
        ("zones", "city_id", "uuid", "NO"),
        ("salary_ranges", "currency", "character", "NO"),
        ("salary_ranges", "min_amount", "numeric", "NO"),
        ("salary_ranges", "max_amount", "numeric", "YES"),
    ] {
        assert_eq!(
            columna(&db, t, col).await,
            Some((tipo.to_string(), nulo.to_string())),
            "{t}.{col}"
        );
    }
    // `zones.zone_id` era una FK a sí misma en el diagrama original; ahora es `city_id`.
    assert_eq!(columna(&db, "zones", "zone_id").await, None);
    // La cadena geográfica no repite el país: la ciudad lo hereda de su estado.
    assert_eq!(columna(&db, "cities", "country_id").await, None);
    db.cleanup().await;
}

/// CT1: bajar la migración nueva quita los diez catálogos y subirla deja el mismo esquema.
#[tokio::test]
async fn ct1_down_y_up_dejan_el_mismo_esquema() {
    let db = TestDb::migrated().await;
    let antes = esquema(&db).await;
    Migrator::down(&db.conn, Some(2)).await.expect("down");
    for t in CATALOGOS {
        assert_eq!(existe(&db, t).await, 0, "down debe quitar {t}");
    }
    // Lo anterior sigue ahí, incluidos los catálogos de la Fase 2.
    for t in ["users", "skills", "professions", "interview_levels"] {
        assert_eq!(existe(&db, t).await, 1, "down no debe tocar {t}");
    }
    // Ni quedan restos: el esquema bajado no menciona ninguna de las tablas nuevas.
    let restos = esquema(&db)
        .await
        .into_iter()
        .filter(|l| CATALOGOS.iter().any(|t| l.contains(&format!("{t}."))))
        .count();
    assert_eq!(restos, 0);
    Migrator::up(&db.conn, None).await.expect("up");
    assert_eq!(esquema(&db).await, antes);
    db.cleanup().await;
}

/// CT2: las FK de la cadena están declaradas en la base, con su destino exacto.
#[tokio::test]
async fn ct2_la_cadena_geografica_tiene_fk() {
    let db = TestDb::migrated().await;
    for (tabla, col, destino) in [
        ("states", "country_id", "countries"),
        ("cities", "state_id", "states"),
        ("zones", "city_id", "cities"),
    ] {
        let n = db
            .count(&format!(
                "SELECT count(*) FROM pg_constraint \
                 WHERE contype = 'f' AND conrelid = '{tabla}'::regclass \
                 AND confrelid = '{destino}'::regclass \
                 AND pg_get_constraintdef(oid) LIKE 'FOREIGN KEY ({col}) REFERENCES {destino}(id)%'"
            ))
            .await;
        assert_eq!(n, 1, "{tabla}.{col} debe tener FK a {destino}");
    }
    db.cleanup().await;
}

/// CT2: el comportamiento de la cadena: no hay huérfanos ni borrados que los dejen.
#[tokio::test]
async fn ct2_no_se_crean_huerfanos_ni_se_borra_un_padre_con_hijos() {
    let db = TestDb::migrated().await;
    let hijo = |tabla: &str, col: &str, padre: &str| {
        format!(
            "INSERT INTO {tabla} (id, code, name, {col}) \
             VALUES ('{}', 'zz-prueba', 'Prueba', '{padre}')",
            uuid::Uuid::now_v7()
        )
    };
    let fantasma = uuid::Uuid::now_v7();
    for (t, c) in [
        ("states", "country_id"),
        ("cities", "state_id"),
        ("zones", "city_id"),
    ] {
        let err = error_de(db.exec(&hijo(t, c, &fantasma.to_string())).await);
        assert!(err.contains("foreign key"), "{t}: {err}");
    }

    // Una cadena válida y luego intentar borrar cada eslabón con hijos.
    let pais = uuid::Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO countries (id, code, name) VALUES ('{pais}', 'zz', 'Zedlandia')"
    ))
    .await
    .unwrap();
    db.exec(&hijo("states", "country_id", &pais.to_string()))
        .await
        .unwrap();
    let estado = common::uuid_de(&db, "SELECT id FROM states WHERE code = 'zz-prueba'").await;
    db.exec(&hijo("cities", "state_id", &estado.to_string()))
        .await
        .unwrap();
    let ciudad = common::uuid_de(&db, "SELECT id FROM cities WHERE code = 'zz-prueba'").await;
    db.exec(&hijo("zones", "city_id", &ciudad.to_string()))
        .await
        .unwrap();
    for (t, id) in [("cities", ciudad), ("states", estado), ("countries", pais)] {
        let err = error_de(db.exec(&format!("DELETE FROM {t} WHERE id = '{id}'")).await);
        assert!(err.contains("foreign key"), "{t}: {err}");
    }
    db.cleanup().await;
}

/// CT3: `code` repetido falla en cada catálogo; y el mismo nombre dentro de un padre también
/// (sin duplicados de texto libre), pero el mismo nombre en padres distintos sí se permite.
#[tokio::test]
async fn ct3_codes_y_nombres_no_se_repiten() {
    let db = TestDb::migrated().await;
    for t in ["salary_ranges", "working_shifts"] {
        // salary_ranges exige montos; los demás solo la forma estándar.
        let (cols, vals) = if t == "salary_ranges" {
            (", currency, min_amount", ", 'GTQ', 0")
        } else {
            ("", "")
        };
        let ins = |id: uuid::Uuid| {
            format!(
                "INSERT INTO {t} (id, code, name{cols}) VALUES ('{id}', 'dup-prueba', 'Dup'{vals})"
            )
        };
        db.exec(&ins(uuid::Uuid::now_v7())).await.unwrap();
        let err = error_de(db.exec(&ins(uuid::Uuid::now_v7())).await);
        assert!(
            err.contains("unique") || err.contains("duplicate"),
            "{t}: {err}"
        );
    }
    for t in [
        "countries",
        "contract_types",
        "employment_sectors",
        "driver_licenses",
        "membership_types",
    ] {
        let ins = |id: uuid::Uuid| {
            format!("INSERT INTO {t} (id, code, name) VALUES ('{id}', 'dup-prueba', 'Dup')")
        };
        db.exec(&ins(uuid::Uuid::now_v7())).await.unwrap();
        let err = error_de(db.exec(&ins(uuid::Uuid::now_v7())).await);
        assert!(
            err.contains("unique") || err.contains("duplicate"),
            "{t}: {err}"
        );
    }
    // Geografía: mismo code en estados, ciudades y zonas.
    let gt = common::uuid_de(&db, "SELECT id FROM countries WHERE code = 'gt'").await;
    let estado_id = common::uuid_de(&db, "SELECT id FROM states WHERE code = 'guatemala'").await;
    let ciudad_id = common::uuid_de(
        &db,
        "SELECT id FROM cities WHERE code = 'guatemala-guatemala'",
    )
    .await;
    for (t, col, padre) in [
        ("states", "country_id", gt),
        ("cities", "state_id", estado_id),
        ("zones", "city_id", ciudad_id),
    ] {
        let ins = |code: &str, name: &str| {
            format!(
                "INSERT INTO {t} (id, code, name, {col}) \
                 VALUES ('{}', '{code}', '{name}', '{padre}')",
                uuid::Uuid::now_v7()
            )
        };
        db.exec(&ins("zz-uno", "Nombre de prueba")).await.unwrap();
        let err = error_de(db.exec(&ins("zz-uno", "Otro nombre")).await);
        assert!(
            err.contains("unique") || err.contains("duplicate"),
            "{t} code: {err}"
        );
        let err = error_de(db.exec(&ins("zz-dos", "Nombre de prueba")).await);
        assert!(
            err.contains("unique") || err.contains("duplicate"),
            "{t} name: {err}"
        );
    }
    // El mismo nombre bajo otro padre es legítimo (hay varios «San José»).
    let otro = common::uuid_de(&db, "SELECT id FROM states WHERE code = 'peten'").await;
    db.exec(&format!(
        "INSERT INTO cities (id, code, name, state_id) \
         VALUES ('{}', 'zz-otro', 'Nombre de prueba', '{otro}')",
        uuid::Uuid::now_v7()
    ))
    .await
    .unwrap();
    db.cleanup().await;
}

/// CT3: la semilla geográfica es solo Guatemala, con sus 22 departamentos y 340 municipios,
/// y `zones` queda vacía (decisión 9).
#[tokio::test]
async fn ct3_la_geografia_sembrada_es_guatemala() {
    let db = TestDb::migrated().await;
    assert_eq!(db.count("SELECT count(*) FROM countries").await, 1);
    assert_eq!(
        db.count("SELECT count(*) FROM countries WHERE code = 'gt' AND name = 'Guatemala'")
            .await,
        1
    );
    assert_eq!(db.count("SELECT count(*) FROM states").await, 22);
    assert_eq!(
        db.count("SELECT count(*) FROM states s JOIN countries c ON c.id = s.country_id WHERE c.code = 'gt'")
            .await,
        22
    );
    assert_eq!(db.count("SELECT count(*) FROM cities").await, 340);
    assert_eq!(db.count("SELECT count(*) FROM zones").await, 0);
    for (depto, n) in MUNICIPIOS {
        let hay = db
            .count(&format!(
                "SELECT count(*) FROM cities c JOIN states s ON s.id = c.state_id \
                 WHERE s.code = '{depto}'"
            ))
            .await;
        assert_eq!(hay, n, "municipios de {depto}");
    }
    assert_eq!(MUNICIPIOS.iter().map(|m| m.1).sum::<i64>(), 340);
    // La capital: el municipio de Guatemala, en el departamento de Guatemala.
    assert_eq!(
        db.count(
            "SELECT count(*) FROM cities c JOIN states s ON s.id = c.state_id \
             WHERE s.name = 'Guatemala' AND c.name = 'Guatemala'"
        )
        .await,
        1
    );
    // Algunos municipios conocidos, con tilde y todo.
    for (depto, muni) in [
        ("guatemala", "Mixco"),
        ("guatemala", "Villa Nueva"),
        ("sacatepequez", "Antigua Guatemala"),
        ("quetzaltenango", "Quetzaltenango"),
        ("solola", "Panajachel"),
        ("peten", "Flores"),
        ("izabal", "Puerto Barrios"),
        ("escuintla", "Sipacate"),
    ] {
        let n = db
            .count(&format!(
                "SELECT count(*) FROM cities c JOIN states s ON s.id = c.state_id \
                 WHERE s.code = '{depto}' AND c.name = '{muni}'"
            ))
            .await;
        assert_eq!(n, 1, "falta {muni} en {depto}");
    }
    assert_eq!(
        db.count("SELECT count(*) FROM states WHERE NOT active")
            .await
            + db.count("SELECT count(*) FROM cities WHERE NOT active")
                .await,
        0
    );
    db.cleanup().await;
}

/// CT3: las semillas de los demás catálogos.
#[tokio::test]
async fn ct3_los_demas_catalogos_estan_sembrados() {
    let db = TestDb::migrated().await;
    for (t, codes) in [
        ("working_shifts", &["diurna", "mixta", "nocturna"][..]),
        (
            "contract_types",
            &["indefinido", "plazo-fijo", "servicios", "pasantia"][..],
        ),
        (
            "employment_sectors",
            &["technology", "finance", "education", "health", "retail"][..],
        ),
        ("driver_licenses", &["a", "b", "c", "m"][..]),
        ("membership_types", &["business"][..]),
    ] {
        for code in codes {
            let n = db
                .count(&format!("SELECT count(*) FROM {t} WHERE code = '{code}'"))
                .await;
            assert_eq!(n, 1, "falta {t}.{code}");
        }
        assert_eq!(
            db.count(&format!("SELECT count(*) FROM {t} WHERE NOT active"))
                .await,
            0,
            "{t}"
        );
    }
    // Los nombres de los tipos de licencia y de la membresía.
    for (t, code, name) in [
        ("driver_licenses", "a", "Licencia tipo A"),
        ("driver_licenses", "m", "Licencia tipo M"),
        ("membership_types", "business", "Empresarial"),
        ("working_shifts", "nocturna", "Nocturna"),
        ("contract_types", "servicios", "Por servicios (freelance)"),
    ] {
        let n = db
            .count(&format!(
                "SELECT count(*) FROM {t} WHERE code = '{code}' AND name = '{name}'"
            ))
            .await;
        assert_eq!(n, 1, "{t}.{code} debe llamarse {name}");
    }
    // `membership_types.business` coincide con el plan Empresarial de `plans`.
    assert_eq!(
        db.count(
            "SELECT count(*) FROM membership_types m JOIN plans p ON p.code = m.code \
             WHERE m.code = 'business' AND p.name = m.name"
        )
        .await,
        1
    );
    db.cleanup().await;
}

/// CT5: los rangos salariales están en quetzales, ordenados, sin huecos ni solapes, y el último
/// no tiene tope.
#[tokio::test]
async fn ct5_los_rangos_salariales_son_quetzales_y_no_se_solapan() {
    let db = TestDb::migrated().await;
    let filas = db
        .conn
        .query_all(sea_orm::Statement::from_string(
            db.conn.get_database_backend(),
            "SELECT currency::text, min_amount::text, max_amount::text, sort_order \
             FROM salary_ranges ORDER BY sort_order"
                .to_string(),
        ))
        .await
        .unwrap();
    assert!(filas.len() >= 5, "hacen falta rangos");
    let mut anterior_max: Option<f64> = None;
    let mut anterior_orden = i32::MIN;
    for (i, f) in filas.iter().enumerate() {
        let moneda: String = f.try_get_by_index(0).unwrap();
        let min: f64 = f.try_get_by_index::<String>(1).unwrap().parse().unwrap();
        let max: Option<f64> = f
            .try_get_by_index::<Option<String>>(2)
            .unwrap()
            .map(|m| m.parse().unwrap());
        let orden: i32 = f.try_get_by_index(3).unwrap();
        assert_eq!(moneda, "GTQ");
        assert!(orden > anterior_orden, "sort_order estrictamente creciente");
        anterior_orden = orden;
        match anterior_max {
            None => assert_eq!(min, 0.0, "el primero empieza en 0"),
            Some(m) => assert!(min > m, "el rango {i} se solapa con el anterior"),
        }
        if let Some(m) = max {
            assert!(m >= min);
            anterior_max = Some(m);
        } else {
            assert_eq!(i, filas.len() - 1, "solo el último puede no tener tope");
            anterior_max = None;
        }
    }
    assert!(anterior_max.is_none(), "el último rango no tiene tope");
    // La base rechaza un rango invertido y uno negativo.
    let err = error_de(
        db.exec(&format!(
            "INSERT INTO salary_ranges (id, code, name, currency, min_amount, max_amount) \
             VALUES ('{}', 'zz-inv', 'Invertido', 'GTQ', 100, 50)",
            uuid::Uuid::now_v7()
        ))
        .await,
    );
    assert!(err.contains("check"), "{err}");
    let err = error_de(
        db.exec(&format!(
            "INSERT INTO salary_ranges (id, code, name, currency, min_amount, max_amount) \
             VALUES ('{}', 'zz-neg', 'Negativo', 'GTQ', -1, 50)",
            uuid::Uuid::now_v7()
        ))
        .await,
    );
    assert!(err.contains("check"), "{err}");
    db.cleanup().await;
}

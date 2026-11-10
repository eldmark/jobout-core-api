//! Casos de verdad de las ofertas para candidatos (CA1–CA7, paso 5 de
//! docs/working/fase5-ofertas-empresas.md), contra «Ofertas para candidatos» y «No
//! discriminación» de `api.md`. PostgreSQL real, una base por prueba. Se escribieron antes que el
//! código, en rojo.
//!
//! Las empresas, membresías y ofertas se insertan por SQL: estas pruebas no dependen de las rutas
//! del reclutador. Ningún reloj de pared: las fechas salen de `current_date` de la base más un
//! desplazamiento.

mod common;
#[path = "common/empresas.rs"]
mod empresas;

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use empresas::{mundo, Mundo, Persona, Resp};
use jobout_core_api::{app_with_state, state::AppState};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{json, Value};

const OFERTAS: &str = "/api/v1/offers";
const ID_NO: &str = "00000000-0000-7000-8000-000000000000";

#[derive(Clone, Copy, PartialEq)]
enum Membresia {
    Vigente,
    Vencida,
    Ninguna,
}

async fn ids(m: &Mundo, tabla: &str, n: usize) -> Vec<String> {
    m.db.conn
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            format!("SELECT id::text FROM {tabla} WHERE active ORDER BY code LIMIT {n}"),
        ))
        .await
        .unwrap()
        .iter()
        .map(|f| f.try_get_by_index::<String>(0).unwrap())
        .collect()
}

/// Ids de catálogo con los que se arman las ofertas (dos de cada uno para probar los filtros).
struct Cat {
    profession: Vec<String>,
    sector: Vec<String>,
    hierarchy: Vec<String>,
    level: Vec<String>,
    state: Vec<String>,
    city: Vec<String>,
    salary: Vec<String>,
    shift: Vec<String>,
    contract: Vec<String>,
    language: Vec<String>,
    skill: Vec<String>,
    license: Vec<String>,
}

impl Cat {
    async fn cargar(m: &Mundo) -> Self {
        Self {
            profession: ids(m, "professions", 2).await,
            sector: ids(m, "employment_sectors", 2).await,
            hierarchy: ids(m, "job_hierarchies", 2).await,
            level: ids(m, "academic_levels", 2).await,
            state: ids(m, "states", 2).await,
            city: ids(m, "cities", 2).await,
            salary: ids(m, "salary_ranges", 2).await,
            shift: ids(m, "working_shifts", 2).await,
            contract: ids(m, "contract_types", 2).await,
            language: ids(m, "languages", 3).await,
            skill: ids(m, "skills", 3).await,
            license: ids(m, "driver_licenses", 2).await,
        }
    }
}

/// Empresa por SQL. El NIT se guarda como bytes opacos.
async fn empresa(m: &Mundo, nombre: &str, verificada: bool, mem: Membresia) -> String {
    let id = m.texto("SELECT gen_random_uuid()::text").await;
    let pais = m.id_catalogo("countries", None).await;
    let sector = m.id_catalogo("employment_sectors", None).await;
    m.db.exec(&format!(
        "INSERT INTO companies (id, name, tax_id, tax_id_hash, website, phone, country_id, \
                                employment_sector_id, verified_at) \
         VALUES ('{id}', '{nombre}', '\\x01'::bytea, '\\x{}'::bytea, 'https://{}.example', \
                 '+502 5555 9999', '{pais}', '{sector}', {})",
        common::hex(nombre.as_bytes()),
        nombre.to_lowercase().replace(' ', "-"),
        if verificada { "now()" } else { "NULL" }
    ))
    .await
    .unwrap();
    let tipo = m.id_catalogo("membership_types", None).await;
    let (ini, fin) = match mem {
        Membresia::Vigente => ("current_date - 10", "current_date + 100"),
        Membresia::Vencida => ("current_date - 100", "current_date - 1"),
        Membresia::Ninguna => return id,
    };
    m.db.exec(&format!(
        "INSERT INTO memberships (id, company_id, membership_type_id, start_date, expiration_date, status) \
         VALUES (gen_random_uuid(), '{id}', '{tipo}', {ini}, {fin}, 'active')"
    ))
    .await
    .unwrap();
    id
}

/// Qué lleva una oferta de prueba.
struct Datos<'a> {
    empresa: &'a str,
    titulo: &'a str,
    estado: &'a str,
    confidencial: bool,
    /// Días respecto de hoy (negativo = pasado).
    publicada: i32,
    contratacion: i32,
}

fn datos<'a>(empresa: &'a str, titulo: &'a str) -> Datos<'a> {
    Datos {
        empresa,
        titulo,
        estado: "published",
        confidencial: false,
        publicada: -1,
        contratacion: 60,
    }
}

/// Inserta una oferta completa (todos los catálogos, requisitos) y devuelve su id.
async fn oferta(m: &Mundo, c: &Cat, d: Datos<'_>) -> String {
    let id = m.texto("SELECT gen_random_uuid()::text").await;
    let cerrada = if d.estado == "closed" {
        "now()"
    } else {
        "NULL"
    };
    m.db.exec(&format!(
        "INSERT INTO job_offers (id, company_id, name, description, status, is_confidential, \
            is_experience_required, required_experience, main_language_id, schedule, \
            required_availability_to_travel, required_driver_license, vacancies, hiring_date, \
            publication_date, closed_at, profession_id, employment_sector_id, job_hierarchy_id, \
            minimum_academic_level_id, state_id, city_id, salary_range_id, working_shift_id, \
            contract_type_id, featured) \
         VALUES ('{id}', '{}', '{}', 'Construirás APIs en Rust para el equipo de producto.', \
            '{}', {}, true, 2, '{}', 'Lunes a viernes, 8 a 17', false, true, 2, \
            current_date + {}, current_date + {}, {cerrada}, '{}', '{}', '{}', '{}', '{}', '{}', \
            '{}', '{}', '{}', true)",
        d.empresa,
        d.titulo,
        d.estado,
        d.confidencial,
        c.language[0],
        d.contratacion,
        d.publicada,
        c.profession[0],
        c.sector[0],
        c.hierarchy[0],
        c.level[0],
        c.state[0],
        c.city[0],
        c.salary[0],
        c.shift[0],
        c.contract[0],
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "INSERT INTO job_offer_skills (job_offer_id, skill_id, is_required, min_level) VALUES \
            ('{id}', '{}', true, 'intermediate'), ('{id}', '{}', false, 'beginner')",
        c.skill[0], c.skill[1]
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "INSERT INTO job_offer_languages (job_offer_id, language_id, min_proficiency) \
         VALUES ('{id}', '{}', 'intermediate')",
        c.language[1]
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "INSERT INTO job_offer_driver_licenses (job_offer_id, driver_license_id) \
         VALUES ('{id}', '{}')",
        c.license[0]
    ))
    .await
    .unwrap();
    id
}

struct Escena {
    m: Mundo,
    c: Cat,
    /// Empresa verificada y vigente, "Café Nuevo".
    emp: String,
    /// Una candidata cualquiera.
    ana: Persona,
}

async fn escena() -> Escena {
    let m = mundo().await;
    let c = Cat::cargar(&m).await;
    let emp = empresa(&m, "Cafe Nuevo", true, Membresia::Vigente).await;
    let ana = m.persona("ana").await;
    Escena { m, c, emp, ana }
}

impl Escena {
    async fn get(&self, ruta: &str) -> Resp {
        self.m.get(ruta, &self.ana).await
    }

    async fn lista(&self, query: &str) -> Value {
        let r = self.get(&format!("{OFERTAS}?{query}")).await;
        assert_eq!(r.status, StatusCode::OK, "{query}: {}", r.text);
        r.json
    }

    async fn ids_de(&self, query: &str) -> Vec<String> {
        self.lista(query).await["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["id"].as_str().unwrap().to_string())
            .collect()
    }

    async fn cerrar(self) {
        self.m.db.cleanup().await;
    }
}

fn codigo(v: &Value) -> &str {
    v["error"].as_str().unwrap_or("")
}

// ================================================================== CA1

/// CA1: el listado trae solo `published` de empresas verificadas con membresía vigente.
#[tokio::test]
async fn ca1_solo_publicadas_de_empresas_verificadas_y_vigentes() {
    let e = escena().await;
    let visible = oferta(&e.m, &e.c, datos(&e.emp, "Visible")).await;
    for estado in ["draft", "paused", "closed"] {
        oferta(
            &e.m,
            &e.c,
            Datos {
                estado,
                ..datos(&e.emp, "Oculta")
            },
        )
        .await;
    }
    let sin_verificar = empresa(&e.m, "Sin Verificar", false, Membresia::Vigente).await;
    let vencida = empresa(&e.m, "Vencida", true, Membresia::Vencida).await;
    let sin_membresia = empresa(&e.m, "Sin Membresia", true, Membresia::Ninguna).await;
    let borrada = empresa(&e.m, "Borrada", true, Membresia::Vigente).await;
    let inactiva = empresa(&e.m, "Inactiva", true, Membresia::Vigente).await;
    e.m.db
        .exec(&format!(
            "UPDATE companies SET deleted_at = now() WHERE id = '{borrada}'"
        ))
        .await
        .unwrap();
    e.m.db
        .exec(&format!(
            "UPDATE companies SET active = false WHERE id = '{inactiva}'"
        ))
        .await
        .unwrap();
    for emp in [
        &sin_verificar,
        &vencida,
        &sin_membresia,
        &borrada,
        &inactiva,
    ] {
        oferta(&e.m, &e.c, datos(emp, "De empresa sin acceso")).await;
    }
    // Una membresía cancelada no vale aunque sus fechas cubran hoy.
    let cancelada = empresa(&e.m, "Cancelada", true, Membresia::Vigente).await;
    e.m.db
        .exec(&format!(
            "UPDATE memberships SET status = 'cancelled' WHERE company_id = '{cancelada}'"
        ))
        .await
        .unwrap();
    oferta(&e.m, &e.c, datos(&cancelada, "De cancelada")).await;
    // Una membresía que aún no empieza tampoco.
    let futura = empresa(&e.m, "Futura", true, Membresia::Vigente).await;
    e.m.db
        .exec(&format!(
            "UPDATE memberships SET start_date = current_date + 1, \
             expiration_date = current_date + 30 WHERE company_id = '{futura}'"
        ))
        .await
        .unwrap();
    oferta(&e.m, &e.c, datos(&futura, "De futura")).await;

    assert_eq!(e.ids_de("").await, [visible]);
    e.cerrar().await;
}

/// La verificación y la vigencia se miran en cada petición: quitarlas oculta al instante y
/// devolverlas muestra otra vez (sin tarea de por medio).
#[tokio::test]
async fn ca1_quitar_la_verificacion_o_la_vigencia_oculta_de_inmediato() {
    let e = escena().await;
    let id = oferta(&e.m, &e.c, datos(&e.emp, "Visible")).await;
    assert_eq!(e.ids_de("").await, std::slice::from_ref(&id));

    e.m.db
        .exec(&format!(
            "UPDATE companies SET verified_at = NULL WHERE id = '{}'",
            e.emp
        ))
        .await
        .unwrap();
    assert!(e.ids_de("").await.is_empty());
    assert_eq!(
        e.get(&format!("{OFERTAS}/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    e.m.db
        .exec(&format!(
            "UPDATE companies SET verified_at = now() WHERE id = '{}'",
            e.emp
        ))
        .await
        .unwrap();
    assert_eq!(e.ids_de("").await, std::slice::from_ref(&id));

    e.m.db
        .exec(&format!(
            "UPDATE memberships SET expiration_date = current_date - 1, \
             start_date = current_date - 20 WHERE company_id = '{}'",
            e.emp
        ))
        .await
        .unwrap();
    assert!(e.ids_de("").await.is_empty());
    assert_eq!(
        e.get(&format!("{OFERTAS}/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    // El último día de vigencia todavía cuenta.
    e.m.db
        .exec(&format!(
            "UPDATE memberships SET expiration_date = current_date WHERE company_id = '{}'",
            e.emp
        ))
        .await
        .unwrap();
    assert_eq!(e.ids_de("").await, [id]);
    e.cerrar().await;
}

/// Una oferta cuya fecha de contratación ya pasó no se ofrece aunque la tarea diaria (OF9) aún no
/// la haya cerrado; la del día sí.
#[tokio::test]
async fn una_oferta_vencida_por_fecha_no_se_ofrece() {
    let e = escena().await;
    let vigente = oferta(
        &e.m,
        &e.c,
        Datos {
            contratacion: 0,
            ..datos(&e.emp, "Hoy")
        },
    )
    .await;
    let vencida = oferta(
        &e.m,
        &e.c,
        Datos {
            contratacion: -1,
            ..datos(&e.emp, "Ayer")
        },
    )
    .await;
    assert_eq!(e.ids_de("").await, [vigente]);
    assert_eq!(
        e.get(&format!("{OFERTAS}/{vencida}")).await.status,
        StatusCode::NOT_FOUND
    );
    e.cerrar().await;
}

/// Nada es público, y cualquier cuenta con sesión puede mirar (un reclutador también).
#[tokio::test]
async fn exige_sesion_y_no_pide_rol() {
    let e = escena().await;
    let id = oferta(&e.m, &e.c, datos(&e.emp, "Visible")).await;
    for ruta in [OFERTAS.to_string(), format!("{OFERTAS}/{id}")] {
        let r = e.m.enviar("GET", &ruta, None, None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{ruta}");
        let r = e.m.enviar("GET", &ruta, Some("basura"), None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{ruta}");
    }
    // Un reclutador de otra empresa mira como cualquiera.
    let otra = empresa(&e.m, "Otra", true, Membresia::Vigente).await;
    let rec = e.m.persona("rec").await;
    e.m.dar_rol(&rec.id, "recruiter").await;
    e.m.db
        .exec(&format!(
            "INSERT INTO company_recruiters (id, user_id, company_id, role, position) \
             VALUES (gen_random_uuid(), '{}', '{otra}', 'recruiter', 'Reclutador')",
            rec.id
        ))
        .await
        .unwrap();
    let r = e.m.get(OFERTAS, &rec).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json["items"].as_array().unwrap().len(), 1);
    // Una cuenta suspendida no entra.
    e.m.db
        .exec(&format!(
            "UPDATE users SET status = 'suspended' WHERE id = '{}'",
            e.ana.id
        ))
        .await
        .unwrap();
    let r = e.get(OFERTAS).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(codigo(&r.json), "cuenta_no_disponible");
    e.cerrar().await;
}

// ============================================== orden y paginación (CA1, CA6)

/// Orden fijo: `publication_date` descendente con `id` de desempate; `oldest` lo invierte.
#[tokio::test]
async fn el_orden_es_por_fecha_con_id_de_desempate() {
    let e = escena().await;
    let mut hechas = Vec::new();
    for (i, dias) in [-5, -5, -5, -2, -9].into_iter().enumerate() {
        let id = oferta(
            &e.m,
            &e.c,
            Datos {
                publicada: dias,
                ..datos(&e.emp, &format!("Oferta {i}"))
            },
        )
        .await;
        hechas.push((dias, id));
    }
    // recent: fecha más nueva primero; en empate, id mayor primero.
    let mut esperado = hechas.clone();
    esperado.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let esperado: Vec<String> = esperado.into_iter().map(|x| x.1).collect();
    assert_eq!(e.ids_de("").await, esperado);
    assert_eq!(e.ids_de("sort=recent").await, esperado);
    // oldest: exactamente al revés.
    let mut al_reves = esperado.clone();
    al_reves.reverse();
    assert_eq!(e.ids_de("sort=oldest").await, al_reves);
    e.cerrar().await;
}

/// Recorrer con cursor trae todas, sin repetir ni saltar, con empates de fecha cruzando páginas,
/// en los dos órdenes; la última página no trae cursor.
#[tokio::test]
async fn paginar_con_cursor_recorre_todo_sin_repetir() {
    let e = escena().await;
    for (i, dias) in [-3, -3, -3, -3, -1, -7, -7].into_iter().enumerate() {
        oferta(
            &e.m,
            &e.c,
            Datos {
                publicada: dias,
                ..datos(&e.emp, &format!("Oferta {i}"))
            },
        )
        .await;
    }
    for sort in ["recent", "oldest"] {
        let todas = e.ids_de(&format!("sort={sort}&limit=50")).await;
        assert_eq!(todas.len(), 7);
        let mut recorridas = Vec::new();
        let mut cursor: Option<String> = None;
        let mut paginas = 0;
        loop {
            let c = cursor
                .as_ref()
                .map_or(String::new(), |c| format!("&cursor={c}"));
            let v = e.lista(&format!("sort={sort}&limit=3{c}")).await;
            let items = v["items"].as_array().unwrap();
            assert!(items.len() <= 3);
            recorridas.extend(items.iter().map(|o| o["id"].as_str().unwrap().to_string()));
            paginas += 1;
            match v["next_cursor"].as_str() {
                Some(c) => cursor = Some(c.to_string()),
                None => break,
            }
            assert!(paginas < 10, "no termina");
        }
        assert_eq!(paginas, 3, "{sort}");
        assert_eq!(recorridas, todas, "{sort}");
    }
    // Una página exacta no deja cursor colgando.
    let v = e.lista("limit=7").await;
    assert!(v["next_cursor"].is_null());
    let v = e.lista("limit=6").await;
    assert!(v["next_cursor"].is_string());
    e.cerrar().await;
}

/// CA6: `limit` 1–50 (defecto 20); fuera de rango o mal escrito → 422 `datos_invalidos`.
#[tokio::test]
async fn ca6_el_limite_tiene_tope() {
    let e = escena().await;
    for i in 0..22 {
        oferta(&e.m, &e.c, datos(&e.emp, &format!("Oferta {i}"))).await;
    }
    assert_eq!(e.ids_de("").await.len(), 20);
    assert_eq!(e.ids_de("limit=1").await.len(), 1);
    assert_eq!(e.ids_de("limit=50").await.len(), 22);
    for q in [
        "limit=0",
        "limit=51",
        "limit=1000",
        "limit=-1",
        "limit=abc",
        "limit=",
        "limit=1.5",
        "limit=1&limit=2",
    ] {
        let r = e.get(&format!("{OFERTAS}?{q}")).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{q}");
        assert_eq!(codigo(&r.json), "datos_invalidos", "{q}");
    }
    e.cerrar().await;
}

/// Un cursor mal formado, vacío, repetido o de otro orden → 422 `datos_invalidos`.
#[tokio::test]
async fn un_cursor_invalido_da_422() {
    let e = escena().await;
    for i in 0..3 {
        oferta(&e.m, &e.c, datos(&e.emp, &format!("Oferta {i}"))).await;
    }
    let v = e.lista("limit=1").await;
    let bueno = v["next_cursor"].as_str().unwrap().to_string();
    for q in [
        "cursor=".to_string(),
        "cursor=no-es-base64!".to_string(),
        "cursor=AAAA".to_string(),
        format!("cursor={bueno}&cursor={bueno}"),
        // Un cursor de `recent` no sirve para `oldest`.
        format!("sort=oldest&cursor={bueno}"),
    ] {
        let r = e.get(&format!("{OFERTAS}?{q}")).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{q}");
        assert_eq!(codigo(&r.json), "datos_invalidos", "{q}");
    }
    // Y el bueno sigue sirviendo.
    assert_eq!(
        e.lista(&format!("limit=1&cursor={bueno}")).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    e.cerrar().await;
}

// ================================================================== CA2

/// CA2: cada filtro de catálogo funciona solo, y se combinan con «y».
#[tokio::test]
async fn ca2_los_filtros_de_catalogo_funcionan() {
    let e = escena().await;
    let base = oferta(&e.m, &e.c, datos(&e.emp, "Base")).await;
    // Una oferta que difiere de la base en todos los catálogos a la vez.
    let otra = oferta(&e.m, &e.c, datos(&e.emp, "Otra")).await;
    // Las ciudades del seed son de varios departamentos: la ciudad nueva va con su departamento.
    let (estado2, ciudad2) = {
        let f =
            e.m.db
                .conn
                .query_one(Statement::from_string(
                    DbBackend::Postgres,
                    format!(
                        "SELECT state_id::text, id::text FROM cities \
                     WHERE state_id <> '{}' ORDER BY code LIMIT 1",
                        e.c.state[0]
                    ),
                ))
                .await
                .unwrap()
                .unwrap();
        (
            f.try_get_by_index::<String>(0).unwrap(),
            f.try_get_by_index::<String>(1).unwrap(),
        )
    };
    e.m.db
        .exec(&format!(
            "UPDATE job_offers SET profession_id = '{}', employment_sector_id = '{}', \
                job_hierarchy_id = '{}', state_id = '{estado2}', city_id = '{ciudad2}', \
                salary_range_id = '{}', working_shift_id = '{}', contract_type_id = '{}' \
             WHERE id = '{otra}'",
            e.c.profession[1],
            e.c.sector[1],
            e.c.hierarchy[1],
            e.c.salary[1],
            e.c.shift[1],
            e.c.contract[1],
        ))
        .await
        .unwrap();
    let filtros = [
        ("profession_id", &e.c.profession),
        ("employment_sector_id", &e.c.sector),
        ("job_hierarchy_id", &e.c.hierarchy),
        ("salary_range_id", &e.c.salary),
        ("working_shift_id", &e.c.shift),
        ("contract_type_id", &e.c.contract),
    ];
    for (nombre, valores) in filtros {
        assert_eq!(
            e.ids_de(&format!("{nombre}={}", valores[0])).await,
            std::slice::from_ref(&base),
            "{nombre}"
        );
        assert_eq!(
            e.ids_de(&format!("{nombre}={}", valores[1])).await,
            std::slice::from_ref(&otra),
            "{nombre}"
        );
    }
    assert_eq!(
        e.ids_de(&format!("state_id={}", e.c.state[0])).await,
        std::slice::from_ref(&base)
    );
    assert_eq!(
        e.ids_de(&format!("state_id={estado2}")).await,
        std::slice::from_ref(&otra)
    );
    assert_eq!(
        e.ids_de(&format!("city_id={}", e.c.city[0])).await,
        std::slice::from_ref(&base)
    );
    assert_eq!(
        e.ids_de(&format!("city_id={ciudad2}")).await,
        std::slice::from_ref(&otra)
    );
    // Combinados: «y», no «o».
    assert_eq!(
        e.ids_de(&format!(
            "profession_id={}&contract_type_id={}&salary_range_id={}&working_shift_id={}",
            e.c.profession[0], e.c.contract[0], e.c.salary[0], e.c.shift[0]
        ))
        .await,
        std::slice::from_ref(&base)
    );
    assert!(e
        .ids_de(&format!(
            "profession_id={}&contract_type_id={}",
            e.c.profession[0], e.c.contract[1]
        ))
        .await
        .is_empty());
    // Un id válido que no existe da una lista vacía, no un error.
    assert!(e.ids_de(&format!("profession_id={ID_NO}")).await.is_empty());
    // Los filtros no abren lo que está oculto.
    e.m.db
        .exec(&format!(
            "UPDATE job_offers SET status = 'paused' WHERE id = '{base}'"
        ))
        .await
        .unwrap();
    assert!(e
        .ids_de(&format!("profession_id={}", e.c.profession[0]))
        .await
        .is_empty());
    e.cerrar().await;
}

/// Un filtro con un valor que no es UUID, vacío o repetido → 422 `datos_invalidos`.
#[tokio::test]
async fn un_filtro_con_valor_ilegal_da_datos_invalidos() {
    let e = escena().await;
    for f in [
        "profession_id",
        "state_id",
        "city_id",
        "contract_type_id",
        "salary_range_id",
        "working_shift_id",
        "employment_sector_id",
        "job_hierarchy_id",
    ] {
        for q in [
            format!("{f}=rust"),
            format!("{f}="),
            format!("{f}={ID_NO}&{f}={ID_NO}"),
            format!("{f}=1"),
        ] {
            let r = e.get(&format!("{OFERTAS}?{q}")).await;
            assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{q}");
            assert_eq!(codigo(&r.json), "datos_invalidos", "{q}");
        }
    }
    e.cerrar().await;
}

/// OF4/CA2: nada por edad, género, estado civil, nombre, foto, empresa ni texto libre, y
/// `sort=match` es de la 5b: 422. El cuerpo no repite el nombre ni el valor recibidos.
#[tokio::test]
async fn parametros_fuera_del_conjunto_dan_parametro_no_permitido() {
    let e = escena().await;
    oferta(&e.m, &e.c, datos(&e.emp, "Visible")).await;
    for q in [
        "age=30",
        "min_age=18",
        "birth_date=1990-01-01",
        "gender=f",
        "gender_preference_id=x",
        "marital_status=soltero",
        "marital_status_id=x",
        "name=zulema",
        "first_name=zulema",
        "photo=si",
        "address=zona1",
        "nationality=gt",
        "document=1234",
        "q=rust",
        "search=rust",
        "company=cafe",
        "company_id=x",
        "company_name=cafe",
        "featured=true",
        "status=draft",
        "skill_id=x",
        "sort=name",
        "sort=company",
        "sort=age",
        "sort=salary",
        "offset=3",
        "page=2",
        "LIMIT=3",
        // Junto a parámetros válidos y a otros inválidos: gana el desconocido.
        "limit=5&age=30",
        "limit=1000&age=30",
        "profession_id=rust&gender=f",
        "age=30&age=31",
    ] {
        let r = e.get(&format!("{OFERTAS}?{q}")).await;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{q}: {}",
            r.text
        );
        assert_eq!(codigo(&r.json), "parametro_no_permitido", "{q}");
        for trozo in q.split(['&', '=']) {
            if trozo.len() > 3 && trozo != "limit" {
                assert!(
                    !r.text.contains(trozo),
                    "el error repite {trozo:?}: {}",
                    r.text
                );
            }
        }
    }
    // La clave también se reconoce codificada en porcentaje.
    let r = e.get(&format!("{OFERTAS}?%61ge=30")).await;
    assert_eq!(codigo(&r.json), "parametro_no_permitido");
    e.cerrar().await;
}

/// `sort=match` (5b) y `sort` vacío son valores ilegales de un parámetro permitido.
#[tokio::test]
async fn sort_match_o_vacio_da_datos_invalidos() {
    let e = escena().await;
    for q in ["sort=match", "sort=", "sort=recent&sort=oldest"] {
        let r = e.get(&format!("{OFERTAS}?{q}")).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{q}");
        assert_eq!(codigo(&r.json), "datos_invalidos", "{q}");
    }
    e.cerrar().await;
}

/// Orden de validación: el 413 del cuerpo va antes que el 422 de la consulta.
#[tokio::test]
async fn un_cuerpo_enorme_en_un_get_da_413() {
    let e = escena().await;
    let r =
        e.m.enviar_crudo(
            "GET",
            &format!("{OFERTAS}?age=30"),
            Some(&e.ana.token),
            vec![b'x'; 9 * 1024],
            &[],
        )
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    e.cerrar().await;
}

// ================================================================== forma

const CLAVES_RESUMEN: [&str; 14] = [
    "city",
    "company",
    "confidential",
    "contract_type",
    "employment_sector",
    "id",
    "my_application",
    "profession",
    "publication_date",
    "salary_range",
    "state",
    "title",
    "working_shift",
    "vacancies",
];

fn claves(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

fn esperadas(extra: &[&str]) -> Vec<String> {
    let mut k: Vec<String> = CLAVES_RESUMEN
        .iter()
        .chain(extra.iter())
        .map(|c| c.to_string())
        .collect();
    k.sort();
    k
}

/// El resumen del listado trae exactamente el contrato: ni más ni menos.
#[tokio::test]
async fn el_resumen_tiene_la_forma_del_contrato() {
    let e = escena().await;
    let id = oferta(&e.m, &e.c, datos(&e.emp, "Desarrollador backend")).await;
    let v = e.lista("").await;
    assert_eq!(claves(&v), ["items", "next_cursor"]);
    let o = &v["items"][0];
    assert_eq!(claves(o), esperadas(&[]));
    assert_eq!(o["id"], id);
    assert_eq!(o["title"], "Desarrollador backend");
    assert_eq!(o["confidential"], false);
    assert_eq!(o["vacancies"], 2);
    let ayer = e.m.texto("SELECT (current_date - 1)::text").await;
    assert_eq!(o["publication_date"], ayer);
    assert_eq!(
        o["company"],
        json!({"id": e.emp, "name": "Cafe Nuevo", "website": "https://cafe-nuevo.example"})
    );
    for (campo, tabla, id) in [
        ("profession", "professions", &e.c.profession[0]),
        ("employment_sector", "employment_sectors", &e.c.sector[0]),
        ("state", "states", &e.c.state[0]),
        ("city", "cities", &e.c.city[0]),
        ("contract_type", "contract_types", &e.c.contract[0]),
        ("salary_range", "salary_ranges", &e.c.salary[0]),
        ("working_shift", "working_shifts", &e.c.shift[0]),
    ] {
        let code =
            e.m.texto(&format!("SELECT code FROM {tabla} WHERE id = '{id}'"))
                .await;
        let name =
            e.m.texto(&format!("SELECT name FROM {tabla} WHERE id = '{id}'"))
                .await;
        assert_eq!(
            o[campo],
            json!({"id": id, "code": code, "name": name}),
            "{campo}"
        );
    }
    // Aún no existen las aplicaciones (paso 6): nadie ha aplicado.
    assert!(o["my_application"].is_null());
    // Los catálogos que la oferta no trae salen `null`, no desaparecen.
    e.m.db
        .exec(&format!(
            "UPDATE job_offers SET salary_range_id = NULL, contract_type_id = NULL, \
                working_shift_id = NULL, employment_sector_id = NULL, city_id = NULL \
             WHERE id = '{id}'"
        ))
        .await
        .unwrap();
    let o = &e.lista("").await["items"][0];
    for campo in [
        "salary_range",
        "contract_type",
        "working_shift",
        "employment_sector",
        "city",
    ] {
        assert!(o[campo].is_null(), "{campo}");
    }
    assert_eq!(claves(o), esperadas(&[]));
    e.cerrar().await;
}

/// El detalle suma los requisitos (y solo eso).
#[tokio::test]
async fn el_detalle_suma_los_requisitos() {
    let e = escena().await;
    let id = oferta(&e.m, &e.c, datos(&e.emp, "Desarrollador backend")).await;
    let r = e.get(&format!("{OFERTAS}/{id}")).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let o = &r.json;
    assert_eq!(
        claves(o),
        esperadas(&[
            "description",
            "driver_licenses",
            "hiring_date",
            "job_hierarchy",
            "languages",
            "main_language",
            "minimum_academic_level",
            "required_availability_to_travel",
            "required_driver_license",
            "required_experience",
            "schedule",
            "skills",
        ])
    );
    assert_eq!(
        o["description"],
        "Construirás APIs en Rust para el equipo de producto."
    );
    assert_eq!(o["schedule"], "Lunes a viernes, 8 a 17");
    let hiring = e.m.texto("SELECT (current_date + 60)::text").await;
    assert_eq!(o["hiring_date"], hiring);
    assert_eq!(o["required_experience"], 2);
    assert_eq!(o["required_availability_to_travel"], false);
    assert_eq!(o["required_driver_license"], true);
    assert_eq!(o["main_language"]["id"], e.c.language[0]);
    assert_eq!(o["job_hierarchy"]["id"], e.c.hierarchy[0]);
    assert_eq!(o["minimum_academic_level"]["id"], e.c.level[0]);
    let habilidades = o["skills"].as_array().unwrap();
    assert_eq!(habilidades.len(), 2);
    // Primero las obligatorias.
    assert_eq!(habilidades[0]["skill"]["id"], e.c.skill[0]);
    assert_eq!(habilidades[0]["is_required"], true);
    assert_eq!(habilidades[0]["min_level"], "intermediate");
    assert_eq!(habilidades[1]["skill"]["id"], e.c.skill[1]);
    assert_eq!(habilidades[1]["is_required"], false);
    assert_eq!(habilidades[1]["min_level"], "beginner");
    assert_eq!(
        claves(&habilidades[0]),
        ["is_required", "min_level", "skill"]
    );
    assert_eq!(claves(&habilidades[0]["skill"]), ["code", "id", "name"]);
    let idiomas = o["languages"].as_array().unwrap();
    assert_eq!(idiomas.len(), 1);
    assert_eq!(idiomas[0]["language"]["id"], e.c.language[1]);
    assert_eq!(idiomas[0]["min_proficiency"], "intermediate");
    assert_eq!(claves(&idiomas[0]), ["language", "min_proficiency"]);
    let licencias = o["driver_licenses"].as_array().unwrap();
    assert_eq!(licencias.len(), 1);
    assert_eq!(licencias[0]["id"], e.c.license[0]);
    assert_eq!(claves(&licencias[0]), ["code", "id", "name"]);
    // Sin requisitos las listas salen vacías, no `null`.
    e.m.db
        .exec(&format!(
            "DELETE FROM job_offer_skills WHERE job_offer_id = '{id}'; \
             DELETE FROM job_offer_languages WHERE job_offer_id = '{id}'; \
             DELETE FROM job_offer_driver_licenses WHERE job_offer_id = '{id}'; \
             UPDATE job_offers SET required_driver_license = false WHERE id = '{id}'"
        ))
        .await
        .unwrap();
    let o = e.get(&format!("{OFERTAS}/{id}")).await.json;
    assert_eq!(o["skills"], json!([]));
    assert_eq!(o["languages"], json!([]));
    assert_eq!(o["driver_licenses"], json!([]));
    assert_eq!(o["required_driver_license"], false);
    e.cerrar().await;
}

/// El texto de la oferta viaja como dato, sin tocar.
#[tokio::test]
async fn el_texto_de_la_oferta_viaja_tal_cual() {
    let e = escena().await;
    let id = oferta(&e.m, &e.c, datos(&e.emp, "Base")).await;
    e.m.db
        .exec(&format!(
            "UPDATE job_offers SET name = '<b>Hola</b> & \"co\"', \
             description = '<script>alert(1)</script> texto largo de la oferta' WHERE id = '{id}'"
        ))
        .await
        .unwrap();
    let o = e.get(&format!("{OFERTAS}/{id}")).await.json;
    assert_eq!(o["title"], "<b>Hola</b> & \"co\"");
    assert_eq!(
        o["description"],
        "<script>alert(1)</script> texto largo de la oferta"
    );
    e.cerrar().await;
}

// ================================================================== CA3

/// CA3: una oferta confidencial no muestra empresa, ni el nombre, el sitio o el id, ni en el
/// listado ni en el detalle; el resto (sector, ubicación, requisitos) sigue visible.
#[tokio::test]
async fn ca3_la_confidencial_no_revela_la_empresa() {
    let e = escena().await;
    let conf = oferta(
        &e.m,
        &e.c,
        Datos {
            confidencial: true,
            ..datos(&e.emp, "Confidencial")
        },
    )
    .await;
    let abierta = oferta(&e.m, &e.c, datos(&e.emp, "Abierta")).await;

    let lista = e.get(OFERTAS).await;
    let detalle = e.get(&format!("{OFERTAS}/{conf}")).await;
    assert_eq!(lista.status, StatusCode::OK);
    assert_eq!(detalle.status, StatusCode::OK);
    let en_lista = lista.json["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == conf)
        .unwrap();
    for o in [en_lista, &detalle.json] {
        assert_eq!(o["confidential"], true);
        assert!(o["company"].is_null());
        assert!(o.as_object().unwrap().contains_key("company"));
        assert_eq!(o["employment_sector"]["id"], e.c.sector[0]);
        assert_eq!(o["state"]["id"], e.c.state[0]);
        assert_eq!(o["city"]["id"], e.c.city[0]);
        assert_eq!(o["salary_range"]["id"], e.c.salary[0]);
    }
    assert_eq!(detalle.json["skills"].as_array().unwrap().len(), 2);
    // Nada de la empresa en ninguna parte de la respuesta.
    for texto in [&detalle.text] {
        for secreto in ["Cafe Nuevo", "cafe-nuevo", &e.emp, "5555 9999"] {
            assert!(!texto.contains(secreto), "se coló {secreto}: {texto}");
        }
    }
    let solo_conf = &lista.text;
    // La no confidencial sí la lleva en el mismo listado.
    assert!(solo_conf.contains(&abierta));
    let v: Value = serde_json::from_str(&lista.text).unwrap();
    let sin_conf: Vec<&Value> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["id"] == conf)
        .collect();
    let texto_conf = serde_json::to_string(&sin_conf).unwrap();
    for secreto in ["Cafe Nuevo", "cafe-nuevo", &e.emp, "5555 9999"] {
        assert!(!texto_conf.contains(secreto), "se coló {secreto}");
    }
    // Un filtro por la empresa no existe: no hay forma de deanonimizar.
    for q in [
        format!("company_id={}", e.emp),
        "company=Cafe%20Nuevo".to_string(),
        "q=Cafe".to_string(),
    ] {
        let r = e.get(&format!("{OFERTAS}?{q}")).await;
        assert_eq!(codigo(&r.json), "parametro_no_permitido", "{q}");
    }
    e.cerrar().await;
}

/// CA3: ningún error nombra a la empresa. El 404 de una oferta confidencial que ya no se ve es
/// el mismo que el de cualquier otra.
#[tokio::test]
async fn ca3_los_errores_no_nombran_la_empresa() {
    let e = escena().await;
    let conf = oferta(
        &e.m,
        &e.c,
        Datos {
            confidencial: true,
            estado: "paused",
            ..datos(&e.emp, "Confidencial")
        },
    )
    .await;
    let r = e.get(&format!("{OFERTAS}/{conf}")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    for secreto in ["Cafe Nuevo", "cafe-nuevo", &e.emp] {
        assert!(!r.text.contains(secreto));
    }
    let otra = e.get(&format!("{OFERTAS}/{ID_NO}")).await;
    assert_eq!(r.text, otra.text);
    e.cerrar().await;
}

// ================================================================== CA4

/// CA4: la respuesta nunca trae datos de reclutadores ni lo interno de la oferta o la empresa.
#[tokio::test]
async fn ca4_no_se_filtran_reclutadores_ni_campos_internos() {
    let e = escena().await;
    let rec =
        e.m.persona_con_email("Reclutadora.Secreta@correo-prueba.com")
            .await;
    e.m.dar_rol(&rec.id, "company_admin").await;
    let cr = e.m.texto("SELECT gen_random_uuid()::text").await;
    e.m.db
        .exec(&format!(
            "INSERT INTO company_recruiters (id, user_id, company_id, role, position) \
             VALUES ('{cr}', '{}', '{}', 'company_admin', 'Reclutadora')",
            rec.id, e.emp
        ))
        .await
        .unwrap();
    let id = oferta(&e.m, &e.c, datos(&e.emp, "Visible")).await;
    e.m.db
        .exec(&format!(
            "UPDATE job_offers SET created_by = '{cr}' WHERE id = '{id}'"
        ))
        .await
        .unwrap();
    let conf = oferta(
        &e.m,
        &e.c,
        Datos {
            confidencial: true,
            ..datos(&e.emp, "Confidencial")
        },
    )
    .await;
    e.m.db
        .exec(&format!(
            "UPDATE job_offers SET created_by = '{cr}' WHERE id = '{conf}'"
        ))
        .await
        .unwrap();
    let mut respuestas = vec![e.get(OFERTAS).await];
    respuestas.push(e.get(&format!("{OFERTAS}/{id}")).await);
    respuestas.push(e.get(&format!("{OFERTAS}/{conf}")).await);
    for r in respuestas {
        assert_eq!(r.status, StatusCode::OK);
        for prohibido in [
            "created_by",
            "recruiter",
            "display_name",
            "first_name",
            "last_name",
            "email",
            "correo-prueba",
            "Reclutadora",
            "status",
            "applications_count",
            "featured",
            "tax_id",
            "phone",
            "is_confidential",
            "company_visible",
            "closed_at",
            "created_at",
            "updated_at",
            "5555 9999",
            &cr,
            &rec.id,
        ] {
            assert!(
                !r.text.contains(prohibido),
                "se coló {prohibido:?}: {}",
                r.text
            );
        }
    }
    // Y la empresa abierta solo lleva id, nombre y sitio.
    let o = e.get(&format!("{OFERTAS}/{id}")).await.json;
    assert_eq!(claves(&o["company"]), ["id", "name", "website"]);
    e.cerrar().await;
}

// ================================================================== CA5

/// CA5: borrador, pausada, cerrada, de empresa sin verificar o vencida, inexistente o con id
/// mal formado → el mismo 404, byte a byte.
#[tokio::test]
async fn ca5_lo_no_visible_da_el_mismo_404() {
    let e = escena().await;
    let mut ocultas = Vec::new();
    for estado in ["draft", "paused", "closed"] {
        ocultas.push(
            oferta(
                &e.m,
                &e.c,
                Datos {
                    estado,
                    ..datos(&e.emp, "Oculta")
                },
            )
            .await,
        );
    }
    for (nombre, verificada, mem) in [
        ("Sin Verificar", false, Membresia::Vigente),
        ("Vencida", true, Membresia::Vencida),
        ("Sin Membresia", true, Membresia::Ninguna),
    ] {
        let emp = empresa(&e.m, nombre, verificada, mem).await;
        ocultas.push(oferta(&e.m, &e.c, datos(&emp, "De otra")).await);
    }
    ocultas.push(ID_NO.to_string());
    ocultas.push("no-es-un-uuid".to_string());
    ocultas.push("1".to_string());
    ocultas.push(ID_NO.to_uppercase());
    let referencia = e.get(&format!("{OFERTAS}/{ID_NO}")).await;
    assert_eq!(referencia.status, StatusCode::NOT_FOUND);
    assert_eq!(codigo(&referencia.json), "no_encontrado");
    for id in ocultas {
        let r = e.get(&format!("{OFERTAS}/{id}")).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{id}");
        assert_eq!(r.text, referencia.text, "{id}");
    }
    e.cerrar().await;
}

// ================================================================== CA7

/// CA7: el listado hace un número fijo de consultas, sin importar cuántas ofertas haya ni cuántos
/// requisitos traigan; el detalle también.
#[tokio::test]
async fn ca7_el_listado_no_hace_n_mas_1() {
    let m = mundo().await;
    let contador = Arc::new(Mutex::new(0usize));
    let c2 = contador.clone();
    let mut conn = m.db.conn.clone();
    conn.set_metric_callback(move |_| {
        *c2.lock().unwrap() += 1;
    });
    let app = app_with_state(AppState {
        db: conn,
        ..empresas::estado(&m.db)
    });
    let m = Mundo { app, ..m };
    let c = Cat::cargar(&m).await;
    let emp = empresa(&m, "Cafe Nuevo", true, Membresia::Vigente).await;
    let ana = m.persona("ana").await;
    let mut medidas = Vec::new();
    let mut ultimo = String::new();
    for tanda in [1, 12] {
        for i in 0..tanda {
            ultimo = oferta(&m, &c, datos(&emp, &format!("Oferta {tanda}-{i}"))).await;
        }
        *contador.lock().unwrap() = 0;
        let r = m.get(&format!("{OFERTAS}?limit=50"), &ana).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(r.json["items"].as_array().unwrap().len() >= tanda);
        medidas.push(*contador.lock().unwrap());
    }
    assert_eq!(
        medidas[0], medidas[1],
        "el listado depende del número de ofertas: {medidas:?}"
    );
    *contador.lock().unwrap() = 0;
    let r = m.get(&format!("{OFERTAS}/{ultimo}"), &ana).await;
    assert_eq!(r.status, StatusCode::OK);
    let detalle = *contador.lock().unwrap();
    assert!(detalle <= 6, "el detalle hizo {detalle} consultas");
    m.db.cleanup().await;
}

/// Mirar ofertas no escribe nada (ni el listado ni el detalle).
#[tokio::test]
async fn mirar_ofertas_no_escribe_nada() {
    let e = escena().await;
    let id = oferta(&e.m, &e.c, datos(&e.emp, "Visible")).await;
    let antes =
        e.m.texto("SELECT md5(string_agg(j::text, ',' ORDER BY id)) FROM job_offers j")
            .await;
    e.lista("limit=5").await;
    e.get(&format!("{OFERTAS}/{id}")).await;
    let despues =
        e.m.texto("SELECT md5(string_agg(j::text, ',' ORDER BY id)) FROM job_offers j")
            .await;
    assert_eq!(antes, despues);
    e.cerrar().await;
}

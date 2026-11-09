//! Casos B1–B8 de la Fase 2 (docs/working/fase2-perfil-cv.md): borrar la cuenta también borra los
//! archivos de los CVs, con una bandeja de reintento (`pending_storage_deletions`).
//!
//! Contra PostgreSQL real, una base por prueba. Los CVs se insertan por SQL y los archivos se
//! guardan directo en el almacén de prueba: no dependen de las rutas de subida.
//!
//! Se escribieron antes que la implementación, en rojo.

mod common;

use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use common::TestDb;
use http_body_util::BodyExt;
use jobout_core_api::{
    app_with_state,
    blob::{new_blob_key, BlobError, BlobStore, MemoryBlobStore},
    crypto::PiiKeys,
    jwt::JwtKeys,
    state::AppState,
    storage_cleanup::{process_pending_deletions, CleanupReport},
};
use sea_orm::{ConnectionTrait, DbBackend, Statement, Value};
use serde_json::{json, Value as Json};
use tower::ServiceExt;

const CLAVE: &str = "una-clave-larga-123";
const EMAIL: &str = "ana.lopez@correo-prueba.com";

/// Almacén de prueba: envuelve a `MemoryBlobStore` y puede fallar al borrar a demanda.
struct FlakyBlobStore {
    inner: MemoryBlobStore,
    fail_delete: AtomicBool,
    delete_calls: AtomicUsize,
}

impl FlakyBlobStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryBlobStore::new(),
            fail_delete: AtomicBool::new(false),
            delete_calls: AtomicUsize::new(0),
        })
    }
    fn fail(&self, on: bool) {
        self.fail_delete.store(on, Ordering::SeqCst);
    }
    fn calls(&self) -> usize {
        self.delete_calls.load(Ordering::SeqCst)
    }
    async fn exists(&self, key: &str) -> bool {
        self.inner.get(key).await.is_ok()
    }
}

#[async_trait]
impl BlobStore for FlakyBlobStore {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), BlobError> {
        self.inner.put(key, bytes).await
    }
    async fn get(&self, key: &str) -> Result<Vec<u8>, BlobError> {
        self.inner.get(key).await
    }
    async fn delete(&self, key: &str) -> Result<(), BlobError> {
        self.delete_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_delete.load(Ordering::SeqCst) {
            return Err(BlobError::Io);
        }
        self.inner.delete(key).await
    }
}

fn pii() -> Arc<PiiKeys> {
    Arc::new(
        PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
    )
}

struct Mundo {
    db: TestDb,
    blobs: Arc<FlakyBlobStore>,
    state: AppState,
    app: Router,
}

async fn mundo() -> Mundo {
    let db = TestDb::migrated().await;
    let blobs = FlakyBlobStore::new();
    let state = AppState {
        db: db.conn.clone(),
        pii: pii(),
        jwt: Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap()),
        blobs: blobs.clone(),
    };
    let app = app_with_state(state.clone());
    Mundo {
        db,
        blobs,
        state,
        app,
    }
}

fn stmt(sql: &str, valores: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, valores)
}

async fn enviar(
    app: &Router,
    metodo: &str,
    uri: &str,
    token: Option<&str>,
    cuerpo: Json,
) -> (StatusCode, Json) {
    let mut peticion = Request::builder()
        .method(metodo)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = token {
        peticion = peticion.header("authorization", format!("Bearer {t}"));
    }
    let r = app
        .clone()
        .oneshot(peticion.body(Body::from(cuerpo.to_string())).unwrap())
        .await
        .unwrap();
    let estado = r.status();
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    (estado, serde_json::from_slice(&bytes).unwrap_or(Json::Null))
}

struct Cuenta {
    token: String,
    user_id: String,
    applicant_id: String,
}

async fn registrar(m: &Mundo, email: &str) -> Cuenta {
    let (estado, c) = enviar(
        &m.app,
        "POST",
        "/api/v1/auth/register",
        None,
        json!({"email": email, "password": CLAVE, "first_name": "Ana", "last_name": "Prueba"}),
    )
    .await;
    assert_eq!(estado, StatusCode::CREATED, "{c}");
    let token = c["access_token"].as_str().unwrap().to_string();
    let user_id = m.state.jwt.validate(&token).unwrap();
    let fila =
        m.db.conn
            .query_one(stmt(
                "SELECT id::text FROM applicants WHERE user_id = $1::uuid",
                vec![user_id.clone().into()],
            ))
            .await
            .unwrap()
            .unwrap();
    Cuenta {
        token,
        user_id,
        applicant_id: fila.try_get_by_index(0).unwrap(),
    }
}

/// Guarda un archivo en el almacén y registra su CV por SQL con la clave cifrada.
/// Devuelve la clave en claro.
async fn agregar_cv(m: &Mundo, c: &Cuenta, version: i32, nombre: &str) -> String {
    let clave = new_blob_key();
    m.blobs.put(&clave, b"%PDF contenido").await.unwrap();
    let cv_id = migration::new_id().to_string();
    m.db.conn
        .execute(stmt(
            "INSERT INTO cvs (id, applicant_id, storage_key, original_filename, mime_type, \
             size_bytes, version, is_primary) \
             VALUES ($1::uuid, $2::uuid, $3, $4, 'application/pdf', 14, $5, $6)",
            vec![
                cv_id.clone().into(),
                c.applicant_id.clone().into(),
                m.state.pii.encrypt(&clave).unwrap().into(),
                nombre.into(),
                version.into(),
                (version == 1).into(),
            ],
        ))
        .await
        .unwrap();
    m.db.conn
        .execute(stmt(
            "INSERT INTO cv_extractions (id, cv_id, extracted, status, model) \
             VALUES ($1::uuid, $2::uuid, '{}'::jsonb, 'ready', 'determinista')",
            vec![migration::new_id().to_string().into(), cv_id.into()],
        ))
        .await
        .unwrap();
    clave
}

async fn borrar(m: &Mundo, c: &Cuenta) -> (StatusCode, Json) {
    enviar(
        &m.app,
        "DELETE",
        "/api/v1/me",
        Some(&c.token),
        json!({"password": CLAVE}),
    )
    .await
}

async fn contar(m: &Mundo, sql: &str) -> i64 {
    m.db.count(sql).await
}

/// Las claves que hay en la bandeja, descifradas.
async fn bandeja(m: &Mundo) -> HashSet<String> {
    let filas =
        m.db.conn
            .query_all(stmt(
                "SELECT storage_key FROM pending_storage_deletions",
                vec![],
            ))
            .await
            .unwrap();
    filas
        .iter()
        .map(|f| {
            let cifrada: Vec<u8> = f.try_get_by_index(0).unwrap();
            m.state.pii.decrypt(&cifrada).unwrap()
        })
        .collect()
}

#[derive(Clone, Default)]
struct Registro(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Registro {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capturar_log() -> (Registro, tracing::subscriber::DefaultGuard) {
    let registro = Registro::default();
    let escritor = registro.clone();
    let guarda = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            // Como en producción: solo lo que emite el servicio (no los spans de sea-orm, que
            // en las pruebas ven los datos que la propia prueba inserta).
            .with_env_filter("jobout_core_api=trace")
            .with_ansi(false)
            .with_writer(move || escritor.clone())
            .finish(),
    );
    (registro, guarda)
}

fn texto(r: &Registro) -> String {
    String::from_utf8_lossy(&r.0.lock().unwrap()).into_owned()
}

// ---------------------------------------------------------------------------------- B1, B2

/// B1: `DELETE /me` borra todos los archivos del candidato y no deja nada en la bandeja.
#[tokio::test]
async fn b1_delete_borra_los_archivos_del_candidato() {
    let m = mundo().await;
    let c = registrar(&m, EMAIL).await;
    let a = agregar_cv(&m, &c, 1, "cv-ana-lopez.pdf").await;
    let b = agregar_cv(&m, &c, 2, "cv-ana-lopez-v2.pdf").await;

    let (estado, _) = borrar(&m, &c).await;

    assert_eq!(estado, StatusCode::NO_CONTENT);
    assert!(!m.blobs.exists(&a).await);
    assert!(!m.blobs.exists(&b).await);
    assert_eq!(
        contar(&m, "SELECT count(*) FROM pending_storage_deletions").await,
        0
    );
    m.db.cleanup().await;
}

/// B2: las filas de CVs, extracciones, perfil y experiencias dejan de existir.
#[tokio::test]
async fn b2_las_filas_del_cv_y_del_perfil_desaparecen() {
    let m = mundo().await;
    let c = registrar(&m, EMAIL).await;
    agregar_cv(&m, &c, 1, "cv.pdf").await;
    let ap = &c.applicant_id;
    m.db.exec(&format!(
        "INSERT INTO professional_data (id, applicant_id, description) \
         VALUES ('{}', '{ap}', 'perfil')",
        migration::new_id()
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "INSERT INTO personal_data (id, applicant_id, phone) VALUES ('{}', '{ap}', '\\x01')",
        migration::new_id()
    ))
    .await
    .unwrap();
    m.db.exec(&format!(
        "INSERT INTO applicant_experiences (id, applicant_id, company, title, start_date) \
         VALUES ('{}', '{ap}', 'Acme', 'Dev', '2020-01-01')",
        migration::new_id()
    ))
    .await
    .unwrap();
    assert_eq!(contar(&m, "SELECT count(*) FROM cv_extractions").await, 1);

    assert_eq!(borrar(&m, &c).await.0, StatusCode::NO_CONTENT);

    for tabla in [
        "users",
        "applicants",
        "cvs",
        "cv_extractions",
        "professional_data",
        "personal_data",
        "applicant_experiences",
    ] {
        assert_eq!(
            contar(&m, &format!("SELECT count(*) FROM {tabla}")).await,
            0,
            "{tabla} conserva filas"
        );
    }
    m.db.cleanup().await;
}

// ------------------------------------------------------------------------------------ B3

/// B3: si el almacén falla, la cuenta sí se borra, la respuesta es 204 y las claves quedan en la
/// bandeja, cifradas.
#[tokio::test]
async fn b3_si_el_almacen_falla_la_cuenta_se_borra_y_las_claves_quedan_en_la_bandeja() {
    let m = mundo().await;
    let c = registrar(&m, EMAIL).await;
    let a = agregar_cv(&m, &c, 1, "cv.pdf").await;
    let b = agregar_cv(&m, &c, 2, "cv2.pdf").await;
    m.blobs.fail(true);

    let (estado, _) = borrar(&m, &c).await;

    assert_eq!(estado, StatusCode::NO_CONTENT);
    assert_eq!(contar(&m, "SELECT count(*) FROM users").await, 0);
    assert_eq!(contar(&m, "SELECT count(*) FROM cvs").await, 0);
    assert_eq!(bandeja(&m).await, HashSet::from([a.clone(), b.clone()]));
    // Cifradas: la clave en claro no aparece en la columna.
    let crudo = common::bytes_de(
        &m.db,
        "SELECT storage_key FROM pending_storage_deletions LIMIT 1",
    )
    .await;
    assert!(!common::contiene(&crudo, a.as_bytes()));
    assert!(!common::contiene(&crudo, b.as_bytes()));
    // Los archivos siguen ahí hasta que el reintento pueda borrarlos.
    assert!(m.blobs.exists(&a).await && m.blobs.exists(&b).await);
    m.db.cleanup().await;
}

// ------------------------------------------------------------------------------- B4, B5

/// B4: el reintento procesa la bandeja, borra los archivos y la vacía; un archivo que ya no
/// existe cuenta como éxito.
#[tokio::test]
async fn b4_el_reintento_vacia_la_bandeja_y_un_archivo_ausente_es_exito() {
    let m = mundo().await;
    let c = registrar(&m, EMAIL).await;
    let a = agregar_cv(&m, &c, 1, "cv.pdf").await;
    let b = agregar_cv(&m, &c, 2, "cv2.pdf").await;
    m.blobs.fail(true);
    borrar(&m, &c).await;

    // Con el almacén aún caído no se pierde nada.
    let informe = process_pending_deletions(&m.state).await.unwrap();
    assert_eq!(
        informe,
        CleanupReport {
            deleted: 0,
            failed: 2,
            flagged_for_review: 0
        }
    );
    assert_eq!(bandeja(&m).await.len(), 2);

    // Una de las dos desaparece por otro lado; el almacén se recupera.
    m.blobs.inner.delete(&a).await.unwrap();
    m.blobs.fail(false);
    let informe = process_pending_deletions(&m.state).await.unwrap();

    assert_eq!(informe.deleted, 2);
    assert_eq!(informe.failed, 0);
    assert!(!m.blobs.exists(&a).await && !m.blobs.exists(&b).await);
    assert_eq!(
        contar(&m, "SELECT count(*) FROM pending_storage_deletions").await,
        0
    );
    m.db.cleanup().await;
}

/// Un fallo anota el intento: `attempts` sube y `last_attempt_at` se llena.
#[tokio::test]
async fn un_fallo_incrementa_attempts_y_marca_la_hora() {
    let m = mundo().await;
    let c = registrar(&m, EMAIL).await;
    agregar_cv(&m, &c, 1, "cv.pdf").await;
    m.blobs.fail(true);
    borrar(&m, &c).await;
    assert_eq!(
        contar(
            &m,
            "SELECT count(*) FROM pending_storage_deletions WHERE last_attempt_at IS NULL"
        )
        .await,
        1
    );

    process_pending_deletions(&m.state).await.unwrap();
    process_pending_deletions(&m.state).await.unwrap();

    assert_eq!(
        contar(&m, "SELECT attempts::bigint FROM pending_storage_deletions").await,
        2
    );
    assert_eq!(
        contar(
            &m,
            "SELECT count(*) FROM pending_storage_deletions WHERE last_attempt_at IS NOT NULL"
        )
        .await,
        1
    );
    m.db.cleanup().await;
}

/// Más de un lote (50) se procesa completo en una pasada.
#[tokio::test]
async fn el_reintento_recorre_varios_lotes() {
    let m = mundo().await;
    let mut claves = vec![];
    for _ in 0..120 {
        let k = new_blob_key();
        m.blobs.put(&k, b"x").await.unwrap();
        m.db.conn
            .execute(stmt(
                "INSERT INTO pending_storage_deletions (id, storage_key) VALUES ($1::uuid, $2)",
                vec![
                    migration::new_id().to_string().into(),
                    m.state.pii.encrypt(&k).unwrap().into(),
                ],
            ))
            .await
            .unwrap();
        claves.push(k);
    }

    let informe = process_pending_deletions(&m.state).await.unwrap();

    assert_eq!(informe.deleted, 120);
    assert_eq!(
        contar(&m, "SELECT count(*) FROM pending_storage_deletions").await,
        0
    );
    for k in claves {
        assert!(!m.blobs.exists(&k).await);
    }
    m.db.cleanup().await;
}

/// B5: tras reintentar, la bandeja no conserva nada de la persona; y mientras una fila espera,
/// no guarda más que la clave cifrada (ni usuario, ni candidato, ni nombre de archivo).
#[tokio::test]
async fn b5_la_bandeja_no_identifica_a_la_persona() {
    let m = mundo().await;
    let c = registrar(&m, EMAIL).await;
    agregar_cv(&m, &c, 1, "cv-ana-lopez.pdf").await;
    m.blobs.fail(true);
    borrar(&m, &c).await;

    let fila =
        m.db.conn
            .query_one(stmt(
                "SELECT t::text FROM pending_storage_deletions t",
                vec![],
            ))
            .await
            .unwrap()
            .unwrap();
    let texto_fila: String = fila.try_get_by_index(0).unwrap();
    for dato in [
        &c.user_id,
        &c.applicant_id,
        "ana",
        "lopez",
        "Prueba",
        ".pdf",
    ] {
        assert!(!texto_fila.contains(dato), "la bandeja contiene {dato:?}");
    }
    assert_eq!(
        contar(&m, "SELECT count(*) FROM pending_storage_deletions WHERE created_by IS NOT NULL OR updated_by IS NOT NULL").await,
        0
    );

    m.blobs.fail(false);
    process_pending_deletions(&m.state).await.unwrap();
    assert_eq!(
        contar(&m, "SELECT count(*) FROM pending_storage_deletions").await,
        0
    );
    m.db.cleanup().await;
}

// ------------------------------------------------------------------------------------ B6

/// B6: la constancia sigue siendo una sola y no lleva datos personales ni claves.
#[tokio::test]
async fn b6_una_sola_constancia_sin_datos_personales() {
    let m = mundo().await;
    let c = registrar(&m, EMAIL).await;
    let k = agregar_cv(&m, &c, 1, "cv-ana-lopez.pdf").await;
    m.blobs.fail(true);
    borrar(&m, &c).await;
    process_pending_deletions(&m.state).await.unwrap();

    assert_eq!(contar(&m, "SELECT count(*) FROM privacy_events").await, 1);
    let fila =
        m.db.conn
            .query_one(stmt("SELECT t::text FROM privacy_events t", vec![]))
            .await
            .unwrap()
            .unwrap();
    let t: String = fila.try_get_by_index(0).unwrap();
    for dato in [
        &c.user_id,
        &c.applicant_id,
        &k,
        "ana",
        "lopez",
        EMAIL,
        ".pdf",
    ] {
        assert!(!t.contains(dato), "la constancia contiene {dato:?}");
    }
    m.db.cleanup().await;
}

/// B6: si una clave no se puede descifrar, no se borra nada: ni cuenta, ni constancia, ni
/// bandeja, ni archivos. Todo o nada.
#[tokio::test]
async fn b6_una_clave_ilegible_deja_la_cuenta_intacta() {
    let m = mundo().await;
    let c = registrar(&m, EMAIL).await;
    let a = agregar_cv(&m, &c, 1, "cv.pdf").await;
    m.db.conn
        .execute(stmt(
            "INSERT INTO cvs (id, applicant_id, storage_key, original_filename, mime_type, \
             size_bytes, version) VALUES ($1::uuid, $2::uuid, '\\xdeadbeef', 'x.pdf', \
             'application/pdf', 1, 2)",
            vec![
                migration::new_id().to_string().into(),
                c.applicant_id.clone().into(),
            ],
        ))
        .await
        .unwrap();

    let (estado, cuerpo) = borrar(&m, &c).await;

    assert_eq!(estado, StatusCode::INTERNAL_SERVER_ERROR, "{cuerpo}");
    assert_eq!(contar(&m, "SELECT count(*) FROM users").await, 1);
    assert_eq!(contar(&m, "SELECT count(*) FROM cvs").await, 2);
    assert_eq!(contar(&m, "SELECT count(*) FROM privacy_events").await, 0);
    assert_eq!(
        contar(&m, "SELECT count(*) FROM pending_storage_deletions").await,
        0
    );
    assert!(m.blobs.exists(&a).await);
    m.db.cleanup().await;
}

/// B6: si falla la escritura en la bandeja, la transacción entera se deshace.
#[tokio::test]
async fn b6_si_falla_la_bandeja_no_queda_nada_a_medias() {
    let m = mundo().await;
    let c = registrar(&m, EMAIL).await;
    let a = agregar_cv(&m, &c, 1, "cv.pdf").await;
    m.db.exec(
        "CREATE FUNCTION falla() RETURNS trigger AS $$ BEGIN RAISE EXCEPTION 'no'; END $$ \
         LANGUAGE plpgsql",
    )
    .await
    .unwrap();
    m.db.exec(
        "CREATE TRIGGER t BEFORE INSERT ON pending_storage_deletions \
         FOR EACH ROW EXECUTE FUNCTION falla()",
    )
    .await
    .unwrap();

    let (estado, _) = borrar(&m, &c).await;

    assert_eq!(estado, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(contar(&m, "SELECT count(*) FROM users").await, 1);
    assert_eq!(contar(&m, "SELECT count(*) FROM cvs").await, 1);
    assert_eq!(contar(&m, "SELECT count(*) FROM privacy_events").await, 0);
    assert!(m.blobs.exists(&a).await);
    m.db.cleanup().await;
}

// ------------------------------------------------------------------------------------ B7

/// B7: los archivos de otro usuario no se tocan, ni en el borrado ni en el reintento, y las
/// claves de dos cuentas borradas no se mezclan.
#[tokio::test]
async fn b7_los_archivos_de_otros_no_se_tocan() {
    let m = mundo().await;
    let ana = registrar(&m, EMAIL).await;
    let luis = registrar(&m, "luis.perez@correo-prueba.com").await;
    let eva = registrar(&m, "eva.soto@correo-prueba.com").await;
    let a1 = agregar_cv(&m, &ana, 1, "a1.pdf").await;
    let a2 = agregar_cv(&m, &ana, 2, "a2.pdf").await;
    let l1 = agregar_cv(&m, &luis, 1, "l1.pdf").await;
    let e1 = agregar_cv(&m, &eva, 1, "e1.pdf").await;

    // Ana se borra con el almacén caído; Luis también.
    m.blobs.fail(true);
    borrar(&m, &ana).await;
    borrar(&m, &luis).await;
    assert_eq!(
        bandeja(&m).await,
        HashSet::from([a1.clone(), a2.clone(), l1.clone()])
    );

    m.blobs.fail(false);
    process_pending_deletions(&m.state).await.unwrap();

    for k in [&a1, &a2, &l1] {
        assert!(!m.blobs.exists(k).await);
    }
    assert!(
        m.blobs.exists(&e1).await,
        "se borró el archivo de otra persona"
    );
    assert_eq!(contar(&m, "SELECT count(*) FROM cvs").await, 1);

    // Y un borrado normal tampoco toca al resto.
    let otro = agregar_cv(&m, &eva, 2, "e2.pdf").await;
    let _ = otro;
    assert_eq!(borrar(&m, &eva).await.0, StatusCode::NO_CONTENT);
    assert!(!m.blobs.exists(&e1).await);
    m.db.cleanup().await;
}

/// B7: borrar a una persona no toca los archivos de quien sigue.
#[tokio::test]
async fn b7_borrar_una_cuenta_deja_los_archivos_de_la_otra() {
    let m = mundo().await;
    let ana = registrar(&m, EMAIL).await;
    let luis = registrar(&m, "luis.perez@correo-prueba.com").await;
    let a = agregar_cv(&m, &ana, 1, "a.pdf").await;
    let l = agregar_cv(&m, &luis, 1, "l.pdf").await;

    assert_eq!(borrar(&m, &ana).await.0, StatusCode::NO_CONTENT);

    assert!(!m.blobs.exists(&a).await);
    assert!(m.blobs.exists(&l).await);
    assert_eq!(contar(&m, "SELECT count(*) FROM cvs").await, 1);
    m.db.cleanup().await;
}

// ------------------------------------------------------------------------------------ B8

/// B8: una clave que falla 10 veces se marca para revisión humana (queda con `attempts = 10`,
/// ya no se reintenta) y el log no contiene la clave ni el nombre del archivo.
#[tokio::test]
async fn b8_diez_fallos_marcan_la_clave_para_revision_sin_loguearla() {
    let (log, _guarda) = capturar_log();
    let m = mundo().await;
    let c = registrar(&m, EMAIL).await;
    let k = agregar_cv(&m, &c, 1, "cv-ana-lopez.pdf").await;
    m.blobs.fail(true);
    borrar(&m, &c).await;

    let mut marcadas = 0;
    for _ in 0..10 {
        marcadas += process_pending_deletions(&m.state)
            .await
            .unwrap()
            .flagged_for_review;
    }
    assert_eq!(
        marcadas, 1,
        "debe marcarse exactamente una vez, en el décimo intento"
    );
    assert_eq!(
        contar(&m, "SELECT attempts::bigint FROM pending_storage_deletions").await,
        10
    );

    // Ya no se reintenta: ni llama al almacén ni cambia el contador, aunque el almacén vuelva.
    let llamadas = m.blobs.calls();
    m.blobs.fail(false);
    let informe = process_pending_deletions(&m.state).await.unwrap();
    assert_eq!(informe, CleanupReport::default());
    assert_eq!(m.blobs.calls(), llamadas);
    assert_eq!(
        contar(&m, "SELECT attempts::bigint FROM pending_storage_deletions").await,
        10
    );
    assert!(m.blobs.exists(&k).await, "la clave marcada no debe tocarse");

    let t = texto(&log);
    assert!(
        t.contains("revisión humana"),
        "falta el aviso de revisión: {t}"
    );
    for secreto in [k.as_str(), "cv-ana-lopez", EMAIL, "ana.lopez"] {
        assert!(!t.contains(secreto), "el log contiene {secreto:?}");
    }
    m.db.cleanup().await;
}

/// El log del borrado con almacén caído tampoco incluye claves ni nombres de archivo.
#[tokio::test]
async fn el_log_del_borrado_no_contiene_claves_ni_nombres() {
    let (log, _guarda) = capturar_log();
    let m = mundo().await;
    let c = registrar(&m, EMAIL).await;
    let k = agregar_cv(&m, &c, 1, "cv-ana-lopez.pdf").await;
    m.blobs.fail(true);

    borrar(&m, &c).await;
    process_pending_deletions(&m.state).await.unwrap();

    let t = texto(&log);
    assert!(!t.is_empty());
    for secreto in [k.as_str(), "cv-ana-lopez", EMAIL, "ana.lopez", CLAVE] {
        assert!(!t.contains(secreto), "el log contiene {secreto:?}");
    }
    m.db.cleanup().await;
}

//! Apoyo para las pruebas de esquema: cada prueba crea su propia base en PostgreSQL real
//! (nunca SQLite: enums, índices parciales y FK se comportan distinto) y la borra al terminar.

use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbErr, Statement};
use uuid::Uuid;

/// URL de administración (la base `postgres`). Se puede cambiar con `TEST_DATABASE_URL`.
pub fn admin_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://jobout:jobout@localhost:5532/postgres".to_string())
}

/// Una base efímera con nombre único. Se borra en `cleanup`.
pub struct TestDb {
    pub conn: DatabaseConnection,
    name: String,
}

impl TestDb {
    /// Crea la base vacía, sin migraciones.
    pub async fn empty() -> Self {
        let admin_url = admin_url();
        let admin = Database::connect(&admin_url)
            .await
            .expect("conectar a admin");
        let name = format!("test_{}", Uuid::now_v7().simple());
        admin
            .execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
            .await
            .expect("crear base de prueba");
        let (base, _) = admin_url.rsplit_once('/').expect("URL con base");
        let conn = Database::connect(format!("{base}/{name}"))
            .await
            .expect("conectar a la base de prueba");
        Self { conn, name }
    }

    /// Base con todas las migraciones aplicadas.
    pub async fn migrated() -> Self {
        let db = Self::empty().await;
        Migrator::up(&db.conn, None).await.expect("migrar up");
        db
    }

    pub async fn exec(&self, sql: &str) -> Result<(), DbErr> {
        self.conn.execute_unprepared(sql).await.map(|_| ())
    }

    /// Cuenta filas de una consulta `SELECT count(*)`.
    pub async fn count(&self, sql: &str) -> i64 {
        let row = self
            .conn
            .query_one(Statement::from_string(
                self.conn.get_database_backend(),
                sql.to_string(),
            ))
            .await
            .expect("consulta")
            .expect("una fila");
        row.try_get_by_index::<i64>(0).expect("count")
    }

    /// Cierra y borra la base.
    pub async fn cleanup(self) {
        let Self { conn, name } = self;
        let _ = conn.close().await;
        let admin = Database::connect(&admin_url()).await.expect("admin");
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
            .await
            .expect("borrar base de prueba");
    }
}

// ---- Ayudas del perfil y los CV (Fase 2). Cada binario de pruebas usa solo algunas.

use jobout_core_api::crypto::PiiKeys;

#[allow(dead_code)]
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Las claves de PII que usan las pruebas de esquema.
#[allow(dead_code)]
pub fn llaves() -> PiiKeys {
    use base64::{engine::general_purpose::STANDARD, Engine};
    PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap()
}

/// Esquema observable: columnas, índices y enums. Sirve para comparar antes/después de migrar.
#[allow(dead_code)]
pub async fn esquema(db: &TestDb) -> Vec<String> {
    let rows = db
        .conn
        .query_all(Statement::from_string(
            db.conn.get_database_backend(),
            "SELECT table_name || '.' || column_name || ':' || data_type || ':' || is_nullable \
             FROM information_schema.columns WHERE table_schema = 'public' \
             UNION ALL SELECT 'idx:' || indexdef FROM pg_indexes WHERE schemaname = 'public' \
             UNION ALL SELECT 'enum:' || t.typname || '=' || e.enumlabel \
             FROM pg_type t JOIN pg_enum e ON e.enumtypid = t.oid \
             UNION ALL SELECT 'con:' || conrelid::regclass::text || ':' || pg_get_constraintdef(oid) \
             FROM pg_constraint WHERE connamespace = 'public'::regnamespace \
             ORDER BY 1"
                .to_string(),
        ))
        .await
        .unwrap();
    rows.iter()
        .map(|r| r.try_get_by_index::<String>(0).unwrap())
        .collect()
}

/// Crea un usuario con su applicant. `tag` hace único el email_hash. Devuelve (user_id, applicant_id).
#[allow(dead_code)]
pub async fn nuevo_applicant(db: &TestDb, tag: &str) -> (Uuid, Uuid) {
    let user = Uuid::now_v7();
    let applicant = Uuid::now_v7();
    db.exec(&format!(
        "INSERT INTO users (id, email, email_hash, first_name, last_name) \
         VALUES ('{user}', '\\x01'::bytea, '\\x{}'::bytea, 'Ana', 'Paz')",
        hex(tag.as_bytes())
    ))
    .await
    .unwrap();
    db.exec(&format!(
        "INSERT INTO applicants (id, user_id) VALUES ('{applicant}', '{user}')"
    ))
    .await
    .unwrap();
    (user, applicant)
}

/// Primer `uuid` de la primera fila de la consulta.
#[allow(dead_code)]
pub async fn uuid_de(db: &TestDb, sql: &str) -> Uuid {
    let row = db
        .conn
        .query_one(Statement::from_string(
            db.conn.get_database_backend(),
            sql.to_string(),
        ))
        .await
        .unwrap()
        .expect("una fila");
    row.try_get_by_index::<Uuid>(0).unwrap()
}

/// Primer `bytea` de la primera fila de la consulta.
#[allow(dead_code)]
pub async fn bytes_de(db: &TestDb, sql: &str) -> Vec<u8> {
    let row = db
        .conn
        .query_one(Statement::from_string(
            db.conn.get_database_backend(),
            sql.to_string(),
        ))
        .await
        .unwrap()
        .expect("una fila");
    row.try_get_by_index::<Vec<u8>>(0).unwrap()
}

/// Verdadero si `aguja` aparece como subsecuencia contigua dentro de `pajar`.
#[allow(dead_code)]
pub fn contiene(pajar: &[u8], aguja: &[u8]) -> bool {
    pajar.windows(aguja.len()).any(|w| w == aguja)
}

/// El mensaje de error de un resultado que debía fallar.
#[allow(dead_code)]
pub fn error_de(r: Result<(), DbErr>) -> String {
    r.expect_err("debía fallar").to_string()
}

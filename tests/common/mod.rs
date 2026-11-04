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

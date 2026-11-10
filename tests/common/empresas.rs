//! Apoyo de las pruebas de empresas, invitaciones y verificación (Fase 5a, paso 3).
//! Cada binario de pruebas lo incluye con `#[path]`; usa solo una parte, de ahí el `allow`.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use http_body_util::BodyExt;
use jobout_core_api::{app_with_state, crypto::PiiKeys, jwt::JwtKeys, state::AppState};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

use crate::common::TestDb;

pub const CLAVE: &str = "una-clave-larga-123";

pub fn pii() -> Arc<PiiKeys> {
    Arc::new(
        PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
    )
}

pub fn jwt() -> Arc<JwtKeys> {
    Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap())
}

pub fn estado(db: &TestDb) -> AppState {
    AppState {
        db: db.conn.clone(),
        pii: pii(),
        jwt: jwt(),
        blobs: jobout_core_api::blob::memory_store(),
        engine: jobout_core_api::interview::default_engine(),
    }
}

pub struct Resp {
    pub status: StatusCode,
    pub json: Value,
    pub text: String,
    pub headers: HeaderMap,
}

#[derive(Clone)]
pub struct Persona {
    pub token: String,
    pub id: String,
    pub email: String,
}

pub struct Mundo {
    pub db: TestDb,
    pub app: Router,
}

pub async fn mundo() -> Mundo {
    let db = TestDb::migrated().await;
    let app = app_with_state(estado(&db));
    Mundo { db, app }
}

impl Mundo {
    pub async fn enviar_crudo(
        &self,
        metodo: &str,
        ruta: &str,
        token: Option<&str>,
        cuerpo: Vec<u8>,
        extra: &[(&str, &str)],
    ) -> Resp {
        let mut p = Request::builder().method(metodo).uri(ruta);
        if let Some(t) = token {
            p = p.header("authorization", format!("Bearer {t}"));
        }
        p = p.header("content-type", "application/json");
        for (k, v) in extra {
            p = p.header(*k, *v);
        }
        let r = self
            .app
            .clone()
            .oneshot(p.body(Body::from(cuerpo)).unwrap())
            .await
            .unwrap();
        let status = r.status();
        let headers = r.headers().clone();
        let bytes = r.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes).to_string();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Resp {
            status,
            json,
            text,
            headers,
        }
    }

    pub async fn enviar(
        &self,
        metodo: &str,
        ruta: &str,
        token: Option<&str>,
        cuerpo: Option<Value>,
    ) -> Resp {
        let bytes = cuerpo
            .map(|c| c.to_string().into_bytes())
            .unwrap_or_default();
        self.enviar_crudo(metodo, ruta, token, bytes, &[]).await
    }

    pub async fn get(&self, ruta: &str, p: &Persona) -> Resp {
        self.enviar("GET", ruta, Some(&p.token), None).await
    }

    pub async fn persona(&self, tag: &str) -> Persona {
        let email = format!("{tag}@correo-prueba.com");
        self.persona_con_email(&email).await
    }

    pub async fn persona_con_email(&self, email: &str) -> Persona {
        let r = self
            .enviar(
                "POST",
                "/api/v1/auth/register",
                None,
                Some(json!({"email": email, "password": CLAVE,
                            "first_name": "Ana", "last_name": "Prueba"})),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "registro: {}", r.text);
        let token = r.json["access_token"].as_str().unwrap().to_string();
        let id = jwt().validate(&token).unwrap();
        Persona {
            token,
            id,
            email: email.to_string(),
        }
    }

    pub async fn dar_rol(&self, user_id: &str, rol: &str) {
        let n = self
            .db
            .conn
            .execute_unprepared(&format!(
                "INSERT INTO user_roles (user_id, role_id) \
                 SELECT '{user_id}', id FROM roles WHERE code = '{rol}'"
            ))
            .await
            .unwrap()
            .rows_affected();
        assert_eq!(n, 1, "el rol {rol} no existe");
    }

    /// Una cuenta con el rol `admin` de plataforma (se siembra, no hay ruta que lo dé).
    pub async fn admin(&self, tag: &str) -> Persona {
        let p = self.persona(tag).await;
        self.dar_rol(&p.id, "admin").await;
        p
    }

    pub async fn id_catalogo(&self, tabla: &str, code: Option<&str>) -> String {
        let filtro = code.map_or(String::new(), |c| format!("AND code = '{c}'"));
        self.texto(&format!(
            "SELECT id::text FROM {tabla} WHERE active {filtro} ORDER BY code LIMIT 1"
        ))
        .await
    }

    pub async fn texto(&self, sql: &str) -> String {
        let fila = self
            .db
            .conn
            .query_one(Statement::from_string(DbBackend::Postgres, sql.to_string()))
            .await
            .unwrap()
            .expect("una fila");
        fila.try_get_by_index::<String>(0).unwrap()
    }

    pub async fn cuerpo_empresa(&self, nit: &str, nombre: &str) -> Value {
        json!({
            "name": nombre,
            "tax_id": nit,
            "website": "https://cafenuevo.example",
            "phone": "+502 5555 0000",
            "description": "Tostamos y vendemos café.",
            "country_id": self.id_catalogo("countries", Some("gt")).await,
            "employment_sector_id": self.id_catalogo("employment_sectors", None).await,
        })
    }

    pub async fn crear_empresa(&self, p: &Persona, nit: &str, nombre: &str) -> Value {
        let r = self
            .enviar(
                "POST",
                "/api/v1/companies",
                Some(&p.token),
                Some(self.cuerpo_empresa(nit, nombre).await),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "crear empresa: {}", r.text);
        r.json
    }

    /// Crea la empresa de una persona nueva y devuelve (persona, id de la empresa).
    pub async fn admin_con_empresa(&self, tag: &str, nit: &str) -> (Persona, String) {
        let p = self.persona(tag).await;
        let c = self.crear_empresa(&p, nit, &format!("Empresa {tag}")).await;
        let id = c["id"].as_str().unwrap().to_string();
        (p, id)
    }

    /// Invita y devuelve el cuerpo del 201 (con el token).
    pub async fn invitar(&self, admin: &Persona, email: &str, rol: &str) -> Value {
        let r = self
            .enviar(
                "POST",
                "/api/v1/company/invitations",
                Some(&admin.token),
                Some(json!({"email": email, "role": rol})),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "invitar: {}", r.text);
        r.json
    }

    pub async fn aceptar(&self, p: &Persona, token: &str) -> Resp {
        self.enviar(
            "POST",
            "/api/v1/invitations/accept",
            Some(&p.token),
            Some(json!({"token": token})),
        )
        .await
    }

    /// Persona nueva que entra a la empresa del admin con el rol dado, por invitación.
    pub async fn miembro(&self, admin: &Persona, tag: &str, rol: &str) -> Persona {
        let p = self.persona(tag).await;
        let inv = self.invitar(admin, &p.email, rol).await;
        let r = self.aceptar(&p, inv["token"].as_str().unwrap()).await;
        assert_eq!(r.status, StatusCode::OK, "aceptar: {}", r.text);
        p
    }

    pub async fn n(&self, sql: &str) -> i64 {
        self.db.count(sql).await
    }
}

pub fn sin_ids(v: &Value) -> Value {
    v.clone()
}

pub fn uuid_valido(s: &str) -> bool {
    Uuid::parse_str(s).is_ok()
}

// --------------------------------------------------------------- log

#[derive(Clone, Default)]
pub struct Registro(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Registro {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Registro {
    pub fn texto(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// Captura el log al máximo (`trace`), el filtro de producción, y deja una marca propia para
/// comprobar que la captura funciona (la prueba de cordura).
pub fn capturar_log() -> (Registro, tracing::subscriber::DefaultGuard) {
    let registro = Registro::default();
    let escritor = registro.clone();
    let guarda = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_env_filter(jobout_core_api::telemetry::env_filter(Some("trace")))
            .with_ansi(false)
            .with_writer(move || escritor.clone())
            .finish(),
    );
    tracing::info!(target: "jobout_core_api::test", "captura-activa");
    (registro, guarda)
}

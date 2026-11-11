//! JB-88 through the real API and a real database: the AI text is what gets stored (encrypted as
//! always), a failing model silently gives the deterministic turn, and nothing that identifies the
//! person reaches the model. The model is a local mock; no network, no key.

#[allow(dead_code)]
mod common;

use std::sync::{Arc, Mutex};

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{Request, StatusCode},
    routing::post,
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use common::TestDb;
use http_body_util::BodyExt;
use jobout_core_api::{
    app_with_state,
    crypto::PiiKeys,
    interview::{OpenRouterConfig, OpenRouterEngine, SharedEngine},
    jwt::JwtKeys,
    state::AppState,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const NOMBRE: &str = "Zelda";
const APELLIDO: &str = "Quintanilla";
const CORREO: &str = "zelda.quintanilla@ejemplo-pii.com";
const CLAVE: &str = "una-clave-larga-123";
const LLAVE: &str = "FAKE-OPENROUTER-KEY-SENTINEL-flow-NEVER-LOG";
const IA_APERTURA: &str = "¡Hola! Qué gusto tenerte aquí. Para empezar, ¿me cuentas por qué te interesa este tipo de trabajo y qué esperas de él?";
const IA_REPREGUNTA: &str = "Gracias por contarlo. ¿Podrías darme un ejemplo concreto, con algún número o resultado, de eso que dices?";

type Cuerpos = Arc<Mutex<Vec<String>>>;

/// A mock that answers with the scripted replies in order and then fails with 500.
fn mock(respuestas: Vec<&'static str>) -> (String, Cuerpos) {
    let cuerpos: Cuerpos = Arc::default();
    let cola = Arc::new(Mutex::new(std::collections::VecDeque::from(respuestas)));
    let (tx, rx) = std::sync::mpsc::channel();
    let c2 = cuerpos.clone();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                async fn h(
                    State((cola, cuerpos)): State<(
                        Arc<Mutex<std::collections::VecDeque<&'static str>>>,
                        Cuerpos,
                    )>,
                    body: Bytes,
                ) -> (StatusCode, String) {
                    cuerpos
                        .lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&body).to_string());
                    match cola.lock().unwrap().pop_front() {
                        Some(t) => (
                            StatusCode::OK,
                            json!({"choices":[{"message":{"content": t}}]}).to_string(),
                        ),
                        None => (StatusCode::INTERNAL_SERVER_ERROR, "down".into()),
                    }
                }
                let app = Router::new()
                    .route("/chat/completions", post(h))
                    .with_state((cola, c2));
                let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(l.local_addr().unwrap().port()).unwrap();
                axum::serve(l, app).await.unwrap();
            });
    });
    (format!("http://127.0.0.1:{}", rx.recv().unwrap()), cuerpos)
}

fn motor(url: &str) -> SharedEngine {
    Arc::new(OpenRouterEngine::new(OpenRouterConfig {
        api_key: Some(LLAVE.into()),
        models: vec!["model/one:free".into()],
        timeout: std::time::Duration::from_millis(3000),
        base_url: url.into(),
    }))
}

fn app(db: &TestDb, engine: SharedEngine) -> Router {
    static UNA_VEZ: std::sync::Once = std::sync::Once::new();
    UNA_VEZ.call_once(|| std::env::set_var("ENFORCE_PLAN_LIMITS", "false"));
    app_with_state(AppState {
        db: db.conn.clone(),
        pii: Arc::new(
            PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
        ),
        jwt: Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap()),
        blobs: jobout_core_api::blob::memory_store(),
        engine,
    })
}

async fn pedir(
    app: &Router,
    metodo: &str,
    uri: &str,
    token: Option<&str>,
    cuerpo: Option<Value>,
) -> (StatusCode, Value) {
    let mut r = Request::builder().method(metodo).uri(uri);
    if let Some(t) = token {
        r = r.header("authorization", format!("Bearer {t}"));
    }
    let b = match cuerpo {
        Some(c) => {
            r = r.header("content-type", "application/json");
            Body::from(c.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(r.body(b).unwrap()).await.unwrap();
    let st = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_wording_is_stored_fallback_is_silent_and_no_pii_travels() {
    let (url, cuerpos) = mock(vec![IA_APERTURA, IA_REPREGUNTA]);
    let db = TestDb::migrated().await;
    let app = app(&db, motor(&url));
    let (st, reg) = pedir(
        &app,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(json!({"email": CORREO, "password": CLAVE, "first_name": NOMBRE, "last_name": APELLIDO})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{reg}");
    let token = reg["access_token"].as_str().unwrap().to_string();
    let recruiter = common::uuid_de(&db, "SELECT id FROM ai_recruiters WHERE code = 'ana'").await;
    let nivel = common::uuid_de(&db, "SELECT id FROM interview_levels WHERE code = 'junior'").await;

    // opening: the model's wording
    let (st, e) = pedir(
        &app,
        "POST",
        "/api/v1/interviews",
        Some(&token),
        Some(json!({"recruiter_id": recruiter, "level_id": nivel})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{e}");
    assert_eq!(e["turns"][0]["text"], IA_APERTURA);
    let id = e["id"].as_str().unwrap().to_string();

    // a vague answer: the follow-up, in the model's wording, still a follow_up
    let turns = format!("/api/v1/interviews/{id}/turns");
    let (st, t) = pedir(&app, "POST", &turns, Some(&token), Some(json!({"text": "Pues sí, me gusta mucho trabajar en equipo y aprender cosas nuevas siempre."}))).await;
    assert_eq!(st, StatusCode::CREATED, "{t}");
    assert_eq!(t["turn"]["text"], IA_REPREGUNTA);
    assert_eq!(t["turn"]["kind"], "follow_up");

    // the model is down now: a silent deterministic turn, no error for the person
    let (st, t) = pedir(&app, "POST", &turns, Some(&token), Some(json!({"text": "Hice una app con Rust y PostgreSQL y bajé la latencia de 800 ms a 200 ms."}))).await;
    assert_eq!(st, StatusCode::CREATED, "{t}");
    assert_eq!(t["done"], false);

    // the stored transcript is the final text (decrypted by the API, ciphertext in the table)
    let (_, e) = pedir(
        &app,
        "GET",
        &format!("/api/v1/interviews/{id}"),
        Some(&token),
        None,
    )
    .await;
    let textos: Vec<&str> = e["turns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["text"].as_str().unwrap())
        .collect();
    assert_eq!(textos.len(), 5);
    assert_eq!((textos[0], textos[2]), (IA_APERTURA, IA_REPREGUNTA));
    assert!(!textos[4].is_empty() && textos[4] != IA_APERTURA);
    let crudo = common::bytes_de(
        &db,
        "SELECT decode(string_agg(encode(transcript, 'hex'), ''), 'hex') FROM interview_turns",
    )
    .await;
    assert!(
        !common::contiene(&crudo, "Qué gusto tenerte".as_bytes()),
        "the transcript stays encrypted"
    );

    // what the model received: the role, the questions, the candidate's answers; never the person
    let enviados = cuerpos.lock().unwrap().join("\n");
    assert!(
        enviados.contains("Pues sí, me gusta"),
        "the answers do travel"
    );
    for pii in [NOMBRE, APELLIDO, CORREO, "ejemplo-pii", LLAVE] {
        assert!(!enviados.contains(pii), "{pii:?} reached the model");
    }
    assert_eq!(
        cuerpos.lock().unwrap().len(),
        3,
        "opening, follow-up and the failed third try"
    );
    db.cleanup().await;
}

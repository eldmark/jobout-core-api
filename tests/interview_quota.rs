//! Casos de verdad de la cuota por plan (paso 9 de docs/working/fase3-entrevista.md): Q1–Q7, más
//! el orden de las validaciones de `POST /interviews`, la frontera del mes (con reloj inyectado,
//! sin dormir) y la atomicidad entre la sesión y su fila en `usage_ledger`.
//!
//! Contra PostgreSQL real; una base por prueba. Se escribieron antes que la implementación, en
//! rojo. Ver CONTRIBUTING.md.

#[allow(dead_code)]
mod common;

use std::sync::{Arc, Once};

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
    crypto::PiiKeys,
    interview_store::{self, CreateOutcome},
    jwt::JwtKeys,
    quota::{enforce_from_env_value, QuotaPolicy},
    state::AppState,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

const CLAVE: &str = "una-clave-larga-123";

fn pii() -> Arc<PiiKeys> {
    Arc::new(
        PiiKeys::from_base64(&STANDARD.encode([1u8; 32]), &STANDARD.encode([2u8; 32])).unwrap(),
    )
}

fn jwt() -> Arc<JwtKeys> {
    Arc::new(JwtKeys::from_secret(&"s".repeat(40)).unwrap())
}

/// Estas pruebas miden la cuota: el interruptor de desarrollo no debe estar apagado, aunque
/// quien las corre lo tenga en su entorno.
fn app(db: &TestDb) -> Router {
    static UNA_VEZ: Once = Once::new();
    UNA_VEZ.call_once(|| std::env::remove_var("ENFORCE_PLAN_LIMITS"));
    app_with_state(AppState {
        db: db.conn.clone(),
        pii: pii(),
        jwt: jwt(),
        blobs: jobout_core_api::blob::memory_store(),
        engine: jobout_core_api::interview::default_engine(),
    })
}

async fn enviar(
    app: &Router,
    metodo: &str,
    uri: &str,
    token: Option<&str>,
    cuerpo: Option<Value>,
) -> (StatusCode, Value) {
    let mut peticion = Request::builder().method(metodo).uri(uri);
    if let Some(t) = token {
        peticion = peticion.header("authorization", format!("Bearer {t}"));
    }
    let cuerpo = match cuerpo {
        Some(json) => {
            peticion = peticion.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let r = app
        .clone()
        .oneshot(peticion.body(cuerpo).unwrap())
        .await
        .unwrap();
    let estado = r.status();
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    (
        estado,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn registrar(app: &Router, email: &str) -> String {
    let (estado, c) = enviar(
        app,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(
            json!({"email": email, "password": CLAVE, "first_name": "Ana", "last_name": "Prueba"}),
        ),
    )
    .await;
    assert_eq!(estado, StatusCode::CREATED, "{c}");
    c["access_token"].as_str().unwrap().to_string()
}

struct Mundo {
    db: TestDb,
    app: Router,
    token: String,
    user: Uuid,
    applicant: Uuid,
}

async fn persona(db: &TestDb, app: &Router, email: &str) -> (String, Uuid, Uuid) {
    let token = registrar(app, email).await;
    let user: Uuid = jwt().validate(&token).unwrap().parse().unwrap();
    let applicant = common::uuid_de(
        db,
        &format!("SELECT id FROM applicants WHERE user_id = '{user}'"),
    )
    .await;
    (token, user, applicant)
}

async fn mundo() -> Mundo {
    let db = TestDb::migrated().await;
    let app = app(&db);
    let (token, user, applicant) = persona(&db, &app, "ana.uno@correo-prueba.com").await;
    Mundo {
        db,
        app,
        token,
        user,
        applicant,
    }
}

async fn id_recruiter(db: &TestDb, code: &str) -> Uuid {
    common::uuid_de(
        db,
        &format!("SELECT id FROM ai_recruiters WHERE code = '{code}'"),
    )
    .await
}

async fn id_nivel(db: &TestDb, code: &str) -> Uuid {
    common::uuid_de(
        db,
        &format!("SELECT id FROM interview_levels WHERE code = '{code}'"),
    )
    .await
}

async fn crear_con(app: &Router, token: &str, db: &TestDb, nivel: &str) -> (StatusCode, Value) {
    let r = id_recruiter(db, "ana").await;
    let n = id_nivel(db, nivel).await;
    enviar(
        app,
        "POST",
        "/api/v1/interviews",
        Some(token),
        Some(json!({"recruiter_id": r, "level_id": n})),
    )
    .await
}

async fn crear(w: &Mundo) -> (StatusCode, Value) {
    crear_con(&w.app, &w.token, &w.db, "junior").await
}

async fn abandonar(w: &Mundo, id: &str) {
    let (e, c) = enviar(
        &w.app,
        "POST",
        &format!("/api/v1/interviews/{id}/abandon"),
        Some(&w.token),
        None,
    )
    .await;
    assert_eq!(e, StatusCode::NO_CONTENT, "{c}");
}

/// Crea y abandona: gasta una unidad de cuota y deja libre el cupo de sesión abierta.
async fn gastar_una(w: &Mundo) {
    let (e, c) = crear(w).await;
    assert_eq!(e, StatusCode::CREATED, "{c}");
    abandonar(w, c["id"].as_str().unwrap()).await;
}

/// Cambia el plan activo de la persona.
async fn poner_plan(db: &TestDb, user: Uuid, codigo: &str) {
    db.exec(&format!(
        "UPDATE user_subscriptions SET status = 'cancelled' WHERE user_id = '{user}' AND status = 'active'"
    ))
    .await
    .unwrap();
    db.exec(&format!(
        "INSERT INTO user_subscriptions (id, user_id, plan_id, status, source) \
         SELECT '{}', '{user}', id, 'active', 'paid' FROM plans WHERE code = '{codigo}'",
        Uuid::now_v7()
    ))
    .await
    .unwrap();
}

/// Una fila de uso de la persona; `cuando` es una expresión SQL.
async fn gasto_previo(db: &TestDb, user: Uuid, cuando: &str) {
    db.exec(&format!(
        "INSERT INTO usage_ledger (id, user_id, provider, operation, units, cost_micros, occurred_at) \
         VALUES ('{}', '{user}', 'deterministic', 'interview_session', 1, 0, {cuando})",
        Uuid::now_v7()
    ))
    .await
    .unwrap();
}

async fn cuenta_ledger(db: &TestDb, user: Uuid) -> i64 {
    db.count(&format!(
        "SELECT count(*) FROM usage_ledger WHERE user_id = '{user}' AND operation = 'interview_session'"
    ))
    .await
}

async fn texto_de(db: &TestDb, sql: &str) -> String {
    use sea_orm::ConnectionTrait;
    db.conn
        .query_one(sea_orm::Statement::from_string(
            db.conn.get_database_backend(),
            sql.to_string(),
        ))
        .await
        .unwrap()
        .expect("una fila")
        .try_get_by_index::<String>(0)
        .unwrap()
}

/// El primer instante del mes siguiente, UTC, según la base, en el formato del contrato.
async fn renovacion_esperada(db: &TestDb) -> String {
    texto_de(
        db,
        "SELECT to_char(date_trunc('month', now() AT TIME ZONE 'UTC') + interval '1 month', \
                        'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')",
    )
    .await
}

fn limite(c: &Value) -> bool {
    c["error"] == "limite_del_plan"
}

// ================================================================== Q1

/// Q1: Gratis permite 1 sesión iniciada por mes; la segunda → 403 `limite_del_plan` con `renews_at`.
#[tokio::test]
async fn q1_gratis_permite_una_y_la_segunda_es_403_con_renovacion() {
    let w = mundo().await;
    gastar_una(&w).await;

    let antes = renovacion_esperada(&w.db).await;
    let (e, c) = crear(&w).await;
    let despues = renovacion_esperada(&w.db).await;
    assert_eq!(e, StatusCode::FORBIDDEN, "{c}");
    assert!(limite(&c), "{c}");
    let renews = c["renews_at"].as_str().expect("renews_at");
    assert!(renews == antes || renews == despues, "{renews} vs {antes}");
    assert!(renews.ends_with("-01T00:00:00Z"), "{renews}");
    assert!(c["message"].as_str().unwrap().len() > 5);
    // Nada a medias: ni sesión nueva ni gasto nuevo.
    assert_eq!(
        w.db.count("SELECT count(*) FROM interview_sessions").await,
        1
    );
    assert_eq!(cuenta_ledger(&w.db, w.user).await, 1);
    w.db.cleanup().await;
}

/// Q1b: ocho POST simultáneos con la cuota libre dejan una sola sesión y una sola fila de uso.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn q1b_ocho_peticiones_simultaneas_dejan_una_sesion() {
    let w = mundo().await;
    let mut tareas = Vec::new();
    for _ in 0..8 {
        let (app, token) = (w.app.clone(), w.token.clone());
        let (r, n) = (
            id_recruiter(&w.db, "ana").await,
            id_nivel(&w.db, "junior").await,
        );
        tareas.push(tokio::spawn(async move {
            enviar(
                &app,
                "POST",
                "/api/v1/interviews",
                Some(&token),
                Some(json!({"recruiter_id": r, "level_id": n})),
            )
            .await
        }));
    }
    let mut creadas = 0;
    for t in tareas {
        let (e, c) = t.await.unwrap();
        match e {
            StatusCode::CREATED => creadas += 1,
            StatusCode::CONFLICT => assert_eq!(c["error"], "sesion_en_curso", "{c}"),
            otro => panic!("estado inesperado {otro}: {c}"),
        }
    }
    assert_eq!(creadas, 1);
    assert_eq!(cuenta_ledger(&w.db, w.user).await, 1);
    w.db.cleanup().await;
}

// ================================================================== Q2

/// Q2: Estudiantil permite 10 y Estándar 12; el siguiente es 403.
#[tokio::test]
async fn q2_los_topes_de_estudiantil_y_estandar() {
    for (plan, tope) in [("student", 10), ("standard", 12)] {
        let w = mundo().await;
        poner_plan(&w.db, w.user, plan).await;
        for _ in 0..tope - 1 {
            gasto_previo(&w.db, w.user, "now()").await;
        }
        // La última permitida sale por la ruta real.
        let (e, c) = crear(&w).await;
        assert_eq!(e, StatusCode::CREATED, "{plan}: {c}");
        abandonar(&w, c["id"].as_str().unwrap()).await;
        assert_eq!(cuenta_ledger(&w.db, w.user).await, tope);

        let (e, c) = crear(&w).await;
        assert_eq!(e, StatusCode::FORBIDDEN, "{plan}: {c}");
        assert!(limite(&c), "{c}");
        w.db.cleanup().await;
    }
}

/// Q2b: el tope sale de `plans.monthly_simulations`, no de una constante del handler.
#[tokio::test]
async fn q2b_el_tope_se_lee_de_la_tabla_de_planes() {
    let w = mundo().await;
    w.db.exec("UPDATE plans SET monthly_simulations = 2 WHERE code = 'free'")
        .await
        .unwrap();
    gastar_una(&w).await;
    gastar_una(&w).await;
    let (e, c) = crear(&w).await;
    assert_eq!(e, StatusCode::FORBIDDEN, "{c}");
    assert!(limite(&c), "{c}");
    w.db.cleanup().await;
}

/// Q2c: un plan sin tope (`NULL`) no limita.
#[tokio::test]
async fn q2c_un_plan_sin_tope_no_limita() {
    let w = mundo().await;
    poner_plan(&w.db, w.user, "business").await;
    for _ in 0..15 {
        gasto_previo(&w.db, w.user, "now()").await;
    }
    let (e, c) = crear(&w).await;
    assert_eq!(e, StatusCode::CREATED, "{c}");
    w.db.cleanup().await;
}

// ================================================================== Q3

/// Q3: cada sesión iniciada escribe una fila de uso con proveedor del motor y costo cero.
#[tokio::test]
async fn q3_cada_sesion_iniciada_escribe_su_fila_de_uso() {
    let w = mundo().await;
    let (e, c) = crear(&w).await;
    assert_eq!(e, StatusCode::CREATED, "{c}");
    let sesion: Uuid = c["id"].as_str().unwrap().parse().unwrap();

    assert_eq!(w.db.count("SELECT count(*) FROM usage_ledger").await, 1);
    let fila = texto_de(
        &w.db,
        "SELECT user_id::text || '|' || interview_session_id::text || '|' || provider || '|' || \
                operation || '|' || units::text || '|' || cost_micros::text FROM usage_ledger",
    )
    .await;
    assert_eq!(
        fila,
        format!("{}|{sesion}|deterministic|interview_session|1|0", w.user)
    );
    // Ocurrió dentro del mes en curso y no en el futuro.
    assert_eq!(
        w.db.count(
            "SELECT count(*) FROM usage_ledger WHERE occurred_at <= now() \
             AND date_trunc('month', occurred_at AT TIME ZONE 'UTC') \
               = date_trunc('month', now() AT TIME ZONE 'UTC')"
        )
        .await,
        1
    );
    w.db.cleanup().await;
}

/// Q3b: lo que se rechaza por cuota no escribe uso, y el uso de otra persona no cuenta.
#[tokio::test]
async fn q3b_el_uso_ajeno_no_cuenta_y_el_rechazo_no_escribe() {
    let w = mundo().await;
    let (token_b, user_b, _) = persona(&w.db, &w.app, "luis.dos@correo-prueba.com").await;
    gastar_una(&w).await;

    // B es Gratis y no ha usado nada: puede, aunque A ya agotó su cupo.
    let (e, c) = crear_con(&w.app, &token_b, &w.db, "junior").await;
    assert_eq!(e, StatusCode::CREATED, "{c}");
    assert_eq!(cuenta_ledger(&w.db, user_b).await, 1);

    let (e, _) = crear(&w).await;
    assert_eq!(e, StatusCode::FORBIDDEN);
    assert_eq!(cuenta_ledger(&w.db, w.user).await, 1);
    w.db.cleanup().await;
}

// ================================================================== Q4

/// Q4: abandonar o borrar una sesión no devuelve la cuota.
#[tokio::test]
async fn q4_abandonar_o_borrar_no_devuelve_la_cuota() {
    let w = mundo().await;
    let (e, c) = crear(&w).await;
    assert_eq!(e, StatusCode::CREATED, "{c}");
    let id = c["id"].as_str().unwrap().to_string();

    // Borrar la sesión activa libera el cupo de sesión abierta, no la cuota.
    let (e, _) = enviar(
        &w.app,
        "DELETE",
        &format!("/api/v1/interviews/{id}"),
        Some(&w.token),
        None,
    )
    .await;
    assert_eq!(e, StatusCode::NO_CONTENT);
    let (e, c) = crear(&w).await;
    assert_eq!(e, StatusCode::FORBIDDEN, "borrar no devuelve: {c}");
    assert!(limite(&c), "{c}");
    // La fila de uso sobrevive a la sesión, sin apuntar a ella.
    assert_eq!(cuenta_ledger(&w.db, w.user).await, 1);
    assert_eq!(
        w.db.count("SELECT count(*) FROM usage_ledger WHERE interview_session_id IS NULL")
            .await,
        1
    );
    w.db.cleanup().await;
}

/// Q4b: abandonar tampoco la devuelve.
#[tokio::test]
async fn q4b_abandonar_no_devuelve_la_cuota() {
    let w = mundo().await;
    gastar_una(&w).await;
    let (e, c) = crear(&w).await;
    assert_eq!(e, StatusCode::FORBIDDEN, "abandonar no devuelve: {c}");
    assert!(limite(&c), "{c}");
    w.db.cleanup().await;
}

// ================================================================== Q5

/// Q5: cambiar de plan a mitad de mes aplica el tope nuevo a lo ya usado.
#[tokio::test]
async fn q5_el_cambio_de_plan_aplica_el_tope_nuevo_a_lo_ya_usado() {
    let w = mundo().await;
    // Gratis agotado ...
    gastar_una(&w).await;
    let (e, c) = crear(&w).await;
    assert_eq!(e, StatusCode::FORBIDDEN, "{c}");
    // ... sube a Estudiantil (10): lo usado (1) cuenta, quedan 9.
    poner_plan(&w.db, w.user, "student").await;
    let (e, c) = crear(&w).await;
    assert_eq!(e, StatusCode::CREATED, "{c}");
    abandonar(&w, c["id"].as_str().unwrap()).await;
    assert_eq!(cuenta_ledger(&w.db, w.user).await, 2);

    // Baja a Gratis con 5 usadas: 5 >= 1, bloqueado.
    for _ in 0..3 {
        gasto_previo(&w.db, w.user, "now()").await;
    }
    poner_plan(&w.db, w.user, "free").await;
    let (e, c) = crear(&w).await;
    assert_eq!(e, StatusCode::FORBIDDEN, "{c}");
    assert!(limite(&c), "{c}");

    // Y a Estándar (12) con 5 usadas, vuelve a poder.
    poner_plan(&w.db, w.user, "standard").await;
    let (e, c) = crear(&w).await;
    assert_eq!(e, StatusCode::CREATED, "{c}");
    w.db.cleanup().await;
}

// ================================================================== Q6

/// Q6: sin suscripción activa (ninguna, vencida o cancelada) se trata como Gratis, no ilimitada.
#[tokio::test]
async fn q6_sin_suscripcion_activa_es_gratis() {
    let casos = [
        "DELETE FROM user_subscriptions",
        "UPDATE user_subscriptions SET status = 'expired'",
        "UPDATE user_subscriptions SET status = 'cancelled'",
        // «activa» pero con vencimiento ya pasado
        "UPDATE user_subscriptions SET expires_at = now() - interval '1 day'",
    ];
    for caso in casos {
        let w = mundo().await;
        if caso.contains("expires_at") {
            poner_plan(&w.db, w.user, "standard").await;
        }
        w.db.exec(caso).await.unwrap();
        gastar_una(&w).await;
        let (e, c) = crear(&w).await;
        assert_eq!(e, StatusCode::FORBIDDEN, "{caso}: {c}");
        assert!(limite(&c), "{caso}: {c}");
        w.db.cleanup().await;
    }
}

// ================================================================== Q7

/// Q7 (unitaria): activa por defecto; solo se apaga con un valor explícito de apagado.
#[test]
fn q7_el_interruptor_esta_activo_por_defecto() {
    assert!(enforce_from_env_value(None));
    assert!(enforce_from_env_value(Some("")));
    assert!(enforce_from_env_value(Some("true")));
    assert!(enforce_from_env_value(Some("TRUE")));
    assert!(enforce_from_env_value(Some("1")));
    assert!(enforce_from_env_value(Some("cualquier-cosa")));
    assert!(!enforce_from_env_value(Some("false")));
    assert!(!enforce_from_env_value(Some("FALSE")));
    assert!(!enforce_from_env_value(Some(" False ")));
    assert!(!enforce_from_env_value(Some("0")));
}

// ================================================================== orden de validaciones

/// El orden del contrato: cuerpo → 404 → nivel_no_disponible → nivel_bloqueado →
/// sesion_en_curso → limite_del_plan. Con la cuota agotada, cada uno gana a la cuota.
#[tokio::test]
async fn la_cuota_es_la_ultima_validacion() {
    let w = mundo().await;
    gastar_una(&w).await; // cuota agotada
    let r = id_recruiter(&w.db, "ana").await;
    let post = |cuerpo: Value| {
        let (app, token) = (w.app.clone(), w.token.clone());
        async move {
            enviar(
                &app,
                "POST",
                "/api/v1/interviews",
                Some(&token),
                Some(cuerpo),
            )
            .await
        }
    };

    let (e, c) =
        post(json!({"recruiter_id": "no-es-uuid", "level_id": id_nivel(&w.db, "junior").await}))
            .await;
    assert_eq!(
        (e, c["error"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("datos_invalidos"))
    );

    let (e, c) =
        post(json!({"recruiter_id": Uuid::now_v7(), "level_id": id_nivel(&w.db, "junior").await}))
            .await;
    assert_eq!(
        (e, c["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("no_encontrado"))
    );

    // Nivel no disponible (el catálogo siembra los tres últimos así) gana al bloqueo y a la cuota.
    let no_disponible = common::uuid_de(
        &w.db,
        "SELECT id FROM interview_levels WHERE NOT available ORDER BY sort_order LIMIT 1",
    )
    .await;
    let (e, c) = post(json!({"recruiter_id": r, "level_id": no_disponible})).await;
    assert_eq!(
        (e, c["error"].as_str()),
        (StatusCode::CONFLICT, Some("nivel_no_disponible"))
    );

    // Nivel bloqueado (disponible pero sin desbloquear) gana a la cuota.
    w.db.exec("UPDATE interview_levels SET available = true WHERE code = 'startup'")
        .await
        .unwrap();
    let bloqueado = id_nivel(&w.db, "startup").await;
    let (e, c) = post(json!({"recruiter_id": r, "level_id": bloqueado})).await;
    assert_eq!(
        (e, c["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("nivel_bloqueado")),
        "{c}"
    );

    // Sesión en curso gana a la cuota.
    w.db.exec(&format!(
        "INSERT INTO interview_sessions (id, applicant_id, ai_recruiter_id, interview_level_id, \
            rubric_version_id, status, started_at) \
         SELECT '{}', '{}', r.id, l.id, \
            (SELECT v.id FROM rubric_versions v WHERE v.rubric_id = r.rubric_id AND v.is_current), \
            'paused', now() FROM ai_recruiters r, interview_levels l \
         WHERE r.code = 'ana' AND l.code = 'junior'",
        Uuid::now_v7(),
        w.applicant
    ))
    .await
    .unwrap();
    let (e, c) = crear(&w).await;
    assert_eq!(
        (e, c["error"].as_str()),
        (StatusCode::CONFLICT, Some("sesion_en_curso")),
        "{c}"
    );
    w.db.cleanup().await;
}

// ================================================================== frontera del mes (reloj inyectado)

/// Microsegundos Unix de un instante UTC (algoritmo civil de Howard Hinnant).
fn micros(y: i64, m: i64, d: i64, h: i64, mi: i64, s: i64, us: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let dias = era * 146_097 + doe - 719_468;
    (((dias * 24 + h) * 60 + mi) * 60 + s) * 1_000_000 + us
}

async fn crear_en(w: &Mundo, ahora: i64) -> CreateOutcome {
    interview_store::create_session(
        &w.db.conn,
        &pii(),
        w.applicant,
        id_recruiter(&w.db, "ana").await,
        id_nivel(&w.db, "junior").await,
        "¿Quién eres?",
        QuotaPolicy {
            enforce: true,
            now_micros: ahora,
        },
    )
    .await
    .expect("no debe ser un error")
}

async fn cerrar_abiertas(w: &Mundo) {
    w.db.exec("UPDATE interview_sessions SET status = 'abandoned' WHERE status = 'in_progress'")
        .await
        .unwrap();
}

fn renovacion(r: &CreateOutcome) -> &str {
    match r {
        CreateOutcome::LimitReached { renews_at } => renews_at,
        otro => panic!("se esperaba el límite, no {otro:?}"),
    }
}

/// El último microsegundo de octubre cuenta para octubre; el primer instante de noviembre empieza
/// un mes nuevo; el último de noviembre sigue contando para noviembre.
#[tokio::test]
async fn la_frontera_del_mes_es_utc_y_exacta() {
    let w = mundo().await;
    let fin_oct = micros(2026, 10, 31, 23, 59, 59, 999_999);
    let ini_nov = micros(2026, 11, 1, 0, 0, 0, 0);
    let fin_nov = micros(2026, 11, 30, 23, 59, 59, 999_999);

    assert!(matches!(
        crear_en(&w, fin_oct).await,
        CreateOutcome::Created { .. }
    ));
    cerrar_abiertas(&w).await;
    let r = crear_en(&w, fin_oct).await;
    assert_eq!(renovacion(&r), "2026-11-01T00:00:00Z");

    // Un microsegundo después ya es otro mes: cupo nuevo.
    assert!(matches!(
        crear_en(&w, ini_nov).await,
        CreateOutcome::Created { .. }
    ));
    cerrar_abiertas(&w).await;
    let r = crear_en(&w, ini_nov).await;
    assert_eq!(renovacion(&r), "2026-12-01T00:00:00Z");
    // El uso de las 00:00:00 de noviembre cuenta para todo noviembre, hasta su último microsegundo.
    let r = crear_en(&w, fin_nov).await;
    assert_eq!(renovacion(&r), "2026-12-01T00:00:00Z");
    // Y el de octubre no se arrastra a diciembre.
    let dic = micros(2026, 12, 1, 0, 0, 0, 0);
    assert!(matches!(
        crear_en(&w, dic).await,
        CreateOutcome::Created { .. }
    ));
    w.db.cleanup().await;
}

/// La renovación de diciembre salta de año y la de febrero bisiesto cae el 1 de marzo.
#[tokio::test]
async fn la_renovacion_cruza_anio_y_febrero_bisiesto() {
    let w = mundo().await;
    let dic = micros(2026, 12, 31, 12, 30, 0, 0);
    assert!(matches!(
        crear_en(&w, dic).await,
        CreateOutcome::Created { .. }
    ));
    cerrar_abiertas(&w).await;
    assert_eq!(renovacion(&crear_en(&w, dic).await), "2027-01-01T00:00:00Z");

    let bisiesto = micros(2028, 2, 29, 23, 0, 0, 0);
    assert!(matches!(
        crear_en(&w, bisiesto).await,
        CreateOutcome::Created { .. }
    ));
    cerrar_abiertas(&w).await;
    assert_eq!(
        renovacion(&crear_en(&w, bisiesto).await),
        "2028-03-01T00:00:00Z"
    );
    w.db.cleanup().await;
}

/// Una fila exactamente en el primer instante del mes siguiente es del mes siguiente: no cuenta
/// para el mes en curso.
#[tokio::test]
async fn el_primer_instante_del_mes_siguiente_no_cuenta_en_este_mes() {
    let w = mundo().await;
    w.db.exec(&format!(
        "INSERT INTO usage_ledger (id, user_id, provider, operation, units, cost_micros, occurred_at) \
         VALUES ('{}', '{}', 'deterministic', 'interview_session', 1, 0, '2026-12-01T00:00:00Z')",
        Uuid::now_v7(),
        w.user
    ))
    .await
    .unwrap();
    let nov = micros(2026, 11, 15, 12, 0, 0, 0);
    assert!(matches!(
        crear_en(&w, nov).await,
        CreateOutcome::Created { .. }
    ));
    cerrar_abiertas(&w).await;
    // Y en diciembre sí cuenta (ya hay una de noviembre y otra de las 00:00 de diciembre).
    let dic = micros(2026, 12, 15, 12, 0, 0, 0);
    let r = crear_en(&w, dic).await;
    assert_eq!(renovacion(&r), "2027-01-01T00:00:00Z");
    w.db.cleanup().await;
}

/// El interruptor apagado salta solo la comprobación: el uso se sigue registrando.
#[tokio::test]
async fn con_el_interruptor_apagado_se_registra_pero_no_se_bloquea() {
    let w = mundo().await;
    let ahora = micros(2026, 11, 9, 12, 0, 0, 0);
    for _ in 0..3 {
        let r = interview_store::create_session(
            &w.db.conn,
            &pii(),
            w.applicant,
            id_recruiter(&w.db, "ana").await,
            id_nivel(&w.db, "junior").await,
            "¿Quién eres?",
            QuotaPolicy {
                enforce: false,
                now_micros: ahora,
            },
        )
        .await
        .unwrap();
        assert!(matches!(r, CreateOutcome::Created { .. }));
        cerrar_abiertas(&w).await;
    }
    assert_eq!(cuenta_ledger(&w.db, w.user).await, 3);
    w.db.cleanup().await;
}

// ================================================================== atomicidad

/// La sesión, su primer turno y su fila de uso nacen juntos: si la fila de uso falla, no queda
/// nada (ni sesión, ni turno), y la persona puede reintentar.
#[tokio::test]
async fn la_sesion_y_su_uso_nacen_juntos_o_no_nacen() {
    let w = mundo().await;
    w.db.exec(
        "CREATE FUNCTION falla_uso() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RAISE EXCEPTION 'uso roto'; END $$; \
         CREATE TRIGGER falla_uso BEFORE INSERT ON usage_ledger \
         FOR EACH ROW EXECUTE FUNCTION falla_uso();",
    )
    .await
    .unwrap();

    let (e, c) = crear(&w).await;
    assert_eq!(e, StatusCode::INTERNAL_SERVER_ERROR, "{c}");
    assert_eq!(
        w.db.count("SELECT count(*) FROM interview_sessions").await,
        0
    );
    assert_eq!(w.db.count("SELECT count(*) FROM interview_turns").await, 0);

    w.db.exec("DROP TRIGGER falla_uso ON usage_ledger")
        .await
        .unwrap();
    let (e, c) = crear(&w).await;
    assert_eq!(e, StatusCode::CREATED, "{c}");
    w.db.cleanup().await;
}

/// Carrera real: con una sola unidad de cuota restante, ocho trabajadores crean y abandonan en
/// bucle. Sin importar cómo se intercalen, solo una creación gana y el uso total es el tope.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn peticiones_simultaneas_no_superan_el_tope() {
    for ronda in 0..3 {
        let w = mundo().await;
        poner_plan(&w.db, w.user, "student").await;
        for _ in 0..9 {
            gasto_previo(&w.db, w.user, "now()").await;
        }
        let (r, n) = (
            id_recruiter(&w.db, "ana").await,
            id_nivel(&w.db, "junior").await,
        );
        let mut tareas = Vec::new();
        for _ in 0..8 {
            let (app, token) = (w.app.clone(), w.token.clone());
            tareas.push(tokio::spawn(async move {
                let mut creadas = 0;
                for _ in 0..6 {
                    let (e, c) = enviar(
                        &app,
                        "POST",
                        "/api/v1/interviews",
                        Some(&token),
                        Some(json!({"recruiter_id": r, "level_id": n})),
                    )
                    .await;
                    match e {
                        StatusCode::CREATED => {
                            creadas += 1;
                            let id = c["id"].as_str().unwrap().to_string();
                            let (e, _) = enviar(
                                &app,
                                "POST",
                                &format!("/api/v1/interviews/{id}/abandon"),
                                Some(&token),
                                None,
                            )
                            .await;
                            assert_eq!(e, StatusCode::NO_CONTENT);
                        }
                        StatusCode::CONFLICT => {}
                        StatusCode::FORBIDDEN => {
                            assert_eq!(c["error"], "limite_del_plan");
                            break;
                        }
                        otro => panic!("estado inesperado {otro}: {c}"),
                    }
                }
                creadas
            }));
        }
        let mut total = 0;
        for t in tareas {
            total += t.await.unwrap();
        }
        assert_eq!(total, 1, "ronda {ronda}: solo cabía una más");
        assert_eq!(cuenta_ledger(&w.db, w.user).await, 10, "ronda {ronda}");
        w.db.cleanup().await;
    }
}

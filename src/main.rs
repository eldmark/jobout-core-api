use anyhow::{Context, Result};
use jobout_core_api::{
    app_with_state, blob::LocalBlobStore, crypto::PiiKeys, interview_store, jwt::JwtKeys,
    state::AppState, storage_cleanup,
};
use sea_orm::Database;
use std::sync::Arc;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    // El filtro fija que sea-orm y sqlx nunca pasen de `warn`: ver `telemetry`.
    tracing_subscriber::fmt()
        .with_env_filter(jobout_core_api::telemetry::env_filter(
            std::env::var("RUST_LOG").ok().as_deref(),
        ))
        .init();

    // El servicio no arranca sin claves válidas: los errores nombran la variable, nunca el valor.
    let pii = PiiKeys::from_env().context("claves de cifrado de PII")?;
    let jwt = JwtKeys::from_env().context("secreto de los JWT")?;
    let blobs = LocalBlobStore::from_env().context("directorio de archivos (BLOB_DIR)")?;
    let database_url = std::env::var("DATABASE_URL").context("falta DATABASE_URL")?;
    let db = Database::connect(&database_url)
        .await
        .context("no se pudo conectar a la base de datos")?;

    let state = AppState {
        db,
        pii: Arc::new(pii),
        jwt: Arc::new(jwt),
        blobs: Arc::new(blobs),
        engine: jobout_core_api::interview::default_engine(),
    };

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".into())
        .parse()?;

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("core-api escuchando en {}", listener.local_addr()?);

    let (parar, parada) = tokio::sync::watch::channel(false);
    let limpieza = tokio::spawn(cleanup_loop(state.clone(), parada));

    axum::serve(listener, app_with_state(state))
        .with_graceful_shutdown(shutdown())
        .await?;

    let _ = parar.send(true);
    let _ = limpieza.await;

    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("apagando");
}

/// Cada 60 s reintenta los archivos pendientes de borrar y abandona las entrevistas pausadas de más de 7 días. Un error se registra y no derriba nada.
async fn cleanup_loop(state: AppState, mut parada: tokio::sync::watch::Receiver<bool>) {
    let mut reloj = tokio::time::interval(std::time::Duration::from_secs(60));
    reloj.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = reloj.tick() => {}
            _ = parada.changed() => return,
        }
        match storage_cleanup::process_pending_deletions(&state).await {
            Ok(informe) if informe.deleted + informe.failed > 0 => {
                tracing::info!(
                    borrados = informe.deleted,
                    fallidos = informe.failed,
                    "limpieza de archivos"
                );
            }
            Ok(_) => {}
            Err(_) => tracing::error!(donde = "limpieza_archivos", "error interno"),
        }
        // Una entrevista pausada más de 7 días se abandona sola y libera el cupo (S13).
        match interview_store::abandon_stale_paused(
            &state.db,
            std::time::SystemTime::now(),
            interview_store::PAUSED_MAX_AGE,
        )
        .await
        {
            Ok(0) => {}
            Ok(n) => tracing::info!(abandonadas = n, "limpieza de entrevistas pausadas"),
            Err(_) => tracing::error!(donde = "limpieza_entrevistas", "error interno"),
        }
    }
}

use anyhow::{Context, Result};
use jobout_core_api::{
    app_with_state, blob::LocalBlobStore, crypto::PiiKeys, jwt::JwtKeys, state::AppState,
    storage_cleanup,
};
use sea_orm::Database;
use std::sync::Arc;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "jobout_core_api=debug,tower_http=debug".into()),
        )
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

/// Reintenta cada 60 s los archivos pendientes de borrar. Un error se registra y no derriba nada.
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
    }
}

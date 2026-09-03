use anyhow::Result;
use jobout_core_api::app;
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

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".into())
        .parse()?;

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("core-api escuchando en {}", listener.local_addr()?);

    axum::serve(listener, app())
        .with_graceful_shutdown(shutdown())
        .await?;

    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("apagando");
}

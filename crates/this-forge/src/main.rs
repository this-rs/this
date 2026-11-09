//! `this-forge serve | invite` — la logique vit dans `this_forge::cli` (voir son usage).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    this_forge::cli::run(&args, &mut std::io::stdout()).await
}

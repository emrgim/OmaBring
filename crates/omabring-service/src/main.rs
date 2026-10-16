use std::sync::Arc;

use omabring_core::{default_bind_addr, default_data_dir, App, VERSION};
use omabring_mcp::{router, Server};
use omabring_stores::Registry;

#[tokio::main]
async fn main() {
    let data_dir = default_data_dir();
    let app = match App::open(&data_dir) {
        Ok(app) => Arc::new(app),
        Err(err) => {
            eprintln!("OmaBring could not open its data directory: {err}");
            std::process::exit(1);
        }
    };
    let bind = default_bind_addr();
    let issuer = format!("http://{bind}");
    let server = Server {
        app,
        backend: Arc::new(Registry::live()),
        issuer,
    };
    let listener = match tokio::net::TcpListener::bind(&bind).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("OmaBring service could not listen on {bind}: {err}");
            std::process::exit(1);
        }
    };
    eprintln!("OmaBring service {VERSION} listening on http://{bind}");
    if let Err(err) = axum_serve(listener, router(server)).await {
        eprintln!("OmaBring service stopped: {err}");
        std::process::exit(1);
    }
}

async fn axum_serve(listener: tokio::net::TcpListener, app: axum::Router) -> std::io::Result<()> {
    axum::serve(listener, app)
        .await
        .map_err(|err| std::io::Error::other(err))
}

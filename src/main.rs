#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate log;

use bollard::Docker;
use dotenv::dotenv;
use hyper::Server;
use std::net::SocketAddr;
use tokio::sync::mpsc;

mod utils;
use utils::docker::run_container;

mod config;
use config::Config;

mod req_handler;
use req_handler::MakeReqHandler;

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

lazy_static! {
    static ref DOCKER: Docker = Docker::connect_with_local_defaults().unwrap();
}

#[tokio::main]
async fn main() {
    dotenv().ok();
    env_logger::init();

    let addr = SocketAddr::from(([0, 0, 0, 0], 4567));
    let (tx, mut rx) = mpsc::channel::<Config>(1);
    let mut config = None;
    let server = Server::bind(&addr)
        .serve(MakeReqHandler { tx })
        .with_graceful_shutdown(async {
            config = rx.recv().await;
            info!("Self-update triggered");
        });

    info!("Starting server");
    if let Err(e) = server.await {
        error!("Server error: {}", e);
    }

    if let Some(cfg) = config {
        // To avoid name collisions with itself when running in
        // a container, we simply don't name the spawned container
        run_container(&DOCKER, cfg, false).await.unwrap();
    }
}

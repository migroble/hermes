#[macro_use]
extern crate lazy_static;

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

    let addr = SocketAddr::from(([0, 0, 0, 0], 4567));
    let (tx, mut rx) = mpsc::channel::<Config>(1);
    let mut config = None;
    let server = Server::bind(&addr)
        .serve(MakeReqHandler { tx })
        .with_graceful_shutdown(async {
            config = rx.recv().await;
        });

    if let Err(e) = server.await {
        eprintln!("Server error: {}", e);
    }

    if let Some(cfg) = config {
        run_container(&DOCKER, cfg).await;
    }
}

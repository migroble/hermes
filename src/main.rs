#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate log;

use bollard::Docker;
use dotenv::dotenv;
use hyper::Server;
use std::{
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
};
use tokio::{fs, sync::mpsc};

mod utils;
use utils::docker::run_container;

mod config;
use config::Config;

mod req_handler;
use req_handler::MakeReqHandler;

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

static PKG_NAME: &str = env!("CARGO_PKG_NAME");

lazy_static! {
    static ref DOCKER: Docker = Docker::connect_with_local_defaults().unwrap();
    static ref CONFIGS_DIR: String =
        env::var("CONFIGS_DIR").unwrap_or_else(|_| "configs".to_string());
    static ref REPOS_DIR: String = env::var("REPOS_DIR").unwrap_or_else(|_| "repos".to_string());
    static ref PORT: u16 = env::var("PORT")
        .ok()
        .map(|port| port.parse().ok())
        .flatten()
        .unwrap_or(4567);
}

async fn init_self() {
    let config_file = [&CONFIGS_DIR, PKG_NAME]
        .iter()
        .collect::<PathBuf>()
        .with_extension("toml");
    let config = Config::from_file(config_file).await.unwrap();
    trace!("Initializing self");
    if let Err(why) = run_container(&DOCKER, config).await {
        error!("Failed to start self in init stage: {}", why);
    }
}

async fn init_all() {
    trace!("Initializing");
    let config_files = fs::read_dir(&*CONFIGS_DIR).await;
    match config_files {
        Ok(mut entries) => {
            trace!("Getting directory entries");
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_file() && path.extension().map(|s| s.to_str()).flatten() == Some("toml")
                {
                    let config = Config::from_file(entry.path()).await.unwrap();
                    // We need to clone the name here to use it in the error message
                    let name = config.name.clone();

                    trace!("Initializing {}", name);
                    if let Err(why) = run_container(&DOCKER, config).await {
                        error!("Failed to start container {} in init stage: {}", name, why);
                    }
                } else {
                    trace!("Ignoring directory or non-toml file {:#?}", path);
                }
            }
        }
        Err(why) => error!(
            "Error reading configs directory {:#?}: {}",
            *CONFIGS_DIR, why
        ),
    }
}

async fn start_server() {
    let addr = SocketAddr::from(([0, 0, 0, 0], *PORT));
    let (tx, mut rx) = mpsc::channel::<Config>(1);
    let mut config = None;
    let server = Server::bind(&addr)
        .serve(MakeReqHandler { tx })
        .with_graceful_shutdown(async {
            config = rx.recv().await;
        });

    info!("Starting server");
    if let Err(why) = server.await {
        error!("Server error: {}", why);
    }

    // This is executed when we do a self-update
    if let Some(cfg) = config {
        run_container(&DOCKER, cfg).await.unwrap()
    }
}

enum Init {
    Server,
    AllContainers,
    Itself,
}

#[tokio::main]
async fn main() {
    dotenv().ok();
    env_logger::init();

    let mut args = env::args();
    let mut init = Init::Server;
    while let Some(arg) = args.next() {
        if arg == "--init" {
            init = if args.next() == Some("all".to_string()) {
                Init::AllContainers
            } else {
                Init::Itself
            };

            break;
        }
    }

    let configs_dir = Path::new(&*CONFIGS_DIR);
    if !configs_dir.is_dir() {
        error!("Invalid configs directory {:#?}", configs_dir);
        return;
    }

    match init {
        Init::Itself => init_self().await,
        Init::AllContainers => init_all().await,
        Init::Server => {
            // Validate repos dir
            // We only validate it here because it isn't
            // needed to initialize the containers
            let repos_dir = Path::new(&*REPOS_DIR);
            if !repos_dir.is_dir() {
                error!("Invalid repos directory {:#?}", repos_dir);
                return;
            }

            start_server().await;
        }
    }
}

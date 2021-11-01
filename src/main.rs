#[macro_use]
extern crate lazy_static;

use anyhow::Result;

use bollard::Docker;
use dotenv::dotenv;
use hmac_sha256::HMAC;
use hyper::{
    body::Buf,
    service::{make_service_fn, service_fn},
    Body, Method, Request, Response, Server, StatusCode,
};

use std::{
    env,
    io::Read,
    net::SocketAddr,
    path::{Path, PathBuf},
};

mod utils;
use utils::{
    docker::{build_image, restart_containers},
    git::{clone_or_fetch_repo, KeyPair},
};

mod config;
use config::Config;

lazy_static! {
    static ref SECRET: Vec<u8> = env::var("SECRET_TOKEN")
        .expect("Expected a secret token in the environment")
        .into_bytes();
    static ref SSH_KEY: KeyPair = {
        let key_path = env::var("SSH_KEY").expect("Expected Github SSH key in the environment");
        let private = Path::new(&key_path).to_path_buf();
        let public = private.with_extension("pub");

        KeyPair { public, private }
    };
    static ref CONFIGS_DIR: String = env::var("CONFIGS_DIR").unwrap_or_else(|_| ".".to_string());
    static ref REPOS_DIR: String = env::var("REPOS_DIR").unwrap_or_else(|_| ".".to_string());
}

async fn handle_webhook(req: Request<Body>) -> Result<Response<Body>> {
    match *req.method() {
        Method::POST => {
            let headers = req.headers();
            let get = move |key| headers.get(key).unwrap().to_str().unwrap().to_string();

            let git_sig = get("X-Hub-Signature-256");
            let _event = get("X-Hub-Event");

            let buf = hyper::body::aggregate(req.into_body()).await?;
            let mut reader = buf.reader();
            let mut body = String::new();
            reader.read_to_string(&mut body)?;

            let sig = HMAC::mac(body.as_bytes(), &SECRET);

            if git_sig[7..] == hex::encode(sig) {
                let data = json::parse(&body)?;

                let name = data["name"].as_str().unwrap();
                let repo_path = [&REPOS_DIR, name].iter().collect::<PathBuf>();
                clone_or_fetch_repo(&SSH_KEY, data["ssh_url"].as_str().unwrap(), &repo_path);

                if repo_path.join("Dockerfile").is_file() {
                    let docker = Docker::connect_with_local_defaults().unwrap();
                    build_image(&docker, name, &repo_path).await;

                    let config_path = [&CONFIGS_DIR, name]
                        .iter()
                        .collect::<PathBuf>()
                        .with_extension("toml");
                    if config_path.is_file() {
                        restart_containers(&docker, Config::from_file(config_path).await.unwrap())
                            .await;
                    }
                }

                Ok(Response::new(Body::empty()))
            } else {
                let mut res = Response::default();
                *res.status_mut() = StatusCode::UNAUTHORIZED;
                Ok(res)
            }
        }
        _ => {
            let mut res = Response::default();
            *res.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
            Ok(res)
        }
    }
}

#[tokio::main]
async fn main() {
    dotenv().ok();

    let addr = SocketAddr::from(([0, 0, 0, 0], 4567));
    let service = make_service_fn(|_| async { Ok::<_, hyper::Error>(service_fn(handle_webhook)) });
    let server = Server::bind(&addr).serve(service);

    if let Err(e) = server.await {
        eprintln!("Server error: {}", e);
    }
}

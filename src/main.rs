#[macro_use]
extern crate lazy_static;

use anyhow::Result;
use bollard::Docker;
use dotenv::dotenv;
use hmac_sha256::HMAC;
use hyper::{
    body::{self, Buf},
    service::Service,
    Body, Method, Request, Response, Server, StatusCode,
};
use std::{
    env,
    future::Future,
    io::Read,
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc;

mod utils;
use utils::{
    docker::{build_image, run_container, stop_container},
    git::{clone_or_fetch_repo, KeyPair},
};

mod config;
use config::Config;

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

static PKG_NAME: &str = env!("CARGO_PKG_NAME");

lazy_static! {
    static ref DOCKER: Docker = Docker::connect_with_local_defaults().unwrap();
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

struct Svc {
    tx: mpsc::Sender<Config>,
}

impl Service<Request<Body>> for Svc {
    type Response = Response<Body>;
    type Error = anyhow::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let tx = self.tx.clone();
        Box::pin(async move {
            match *req.method() {
                Method::POST => {
                    let headers = req.headers();
                    let get = move |key| Some(headers.get(key)?.to_str().ok()?.to_string());

                    let git_sig = get("X-Hub-Signature-256").unwrap();
                    let _event = get("X-Hub-Event").unwrap();

                    let buf = body::aggregate(req.into_body()).await?;
                    let mut reader = buf.reader();
                    let mut body = String::new();
                    reader.read_to_string(&mut body)?;

                    let sig = HMAC::mac(body.as_bytes(), &SECRET);

                    if git_sig[7..] == hex::encode(sig) {
                        let data = json::parse(&body)?;

                        let name = data["name"].as_str().unwrap();
                        let repo_path = [&REPOS_DIR, name].iter().collect::<PathBuf>();
                        clone_or_fetch_repo(
                            &SSH_KEY,
                            data["ssh_url"].as_str().unwrap(),
                            &repo_path,
                        );

                        if repo_path.join("Dockerfile").is_file() {
                            build_image(&DOCKER, name, &repo_path).await;

                            let config_path = [&CONFIGS_DIR, name]
                                .iter()
                                .collect::<PathBuf>()
                                .with_extension("toml");
                            if config_path.is_file() {
                                let config = Config::from_file(config_path).await.unwrap();

                                if name == PKG_NAME {
                                    tx.send(config).await.unwrap();
                                } else {
                                    stop_container(&DOCKER, &config).await;
                                    run_container(&DOCKER, config).await;
                                }
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
        })
    }
}

struct MakeSvc {
    tx: mpsc::Sender<Config>,
}

impl<T> Service<T> for MakeSvc {
    type Response = Svc;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: T) -> Self::Future {
        let tx = self.tx.clone();
        let fut = async move { Ok(Svc { tx }) };
        Box::pin(fut)
    }
}

#[tokio::main]
async fn main() {
    dotenv().ok();

    let addr = SocketAddr::from(([0, 0, 0, 0], 4567));
    let (tx, mut rx) = mpsc::channel::<Config>(1);
    let mut config = None;
    let server = Server::bind(&addr)
        .serve(MakeSvc { tx })
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

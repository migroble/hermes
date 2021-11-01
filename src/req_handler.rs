use crate::{
    config::Config,
    utils::{
        docker::{build_image, run_container, stop_container},
        git::{clone_or_fetch_repo, KeyPair},
    },
    DOCKER,
};
use anyhow::Result;
use hmac_sha256::HMAC;
use hyper::{
    body::{self, Buf},
    service::Service,
    Body, Method, Request, Response, StatusCode,
};
use std::{
    env,
    future::Future,
    io::Read,
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc;

static PKG_NAME: &str = env!("CARGO_PKG_NAME");

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

pub struct ReqHandler {
    tx: mpsc::Sender<Config>,
}

impl Service<Request<Body>> for ReqHandler {
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
                        )?;

                        if repo_path.join("Dockerfile").is_file() {
                            build_image(&DOCKER, name, &repo_path).await?;

                            let config_path = [&CONFIGS_DIR, name]
                                .iter()
                                .collect::<PathBuf>()
                                .with_extension("toml");
                            if config_path.is_file() {
                                let config = Config::from_file(config_path).await.unwrap();

                                if name == PKG_NAME {
                                    tx.send(config).await.unwrap();
                                } else {
                                    stop_container(&DOCKER, &config).await?;
                                    run_container(&DOCKER, config).await?;
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

pub struct MakeReqHandler {
    pub tx: mpsc::Sender<Config>,
}

impl<T> Service<T> for MakeReqHandler {
    type Response = ReqHandler;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: T) -> Self::Future {
        let tx = self.tx.clone();
        let fut = async move { Ok(ReqHandler { tx }) };
        Box::pin(fut)
    }
}

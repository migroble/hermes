use crate::{
    config::Config,
    utils::{
        docker::{build_image, find_containers_with_image, run_container, stop_container},
        git::{clone_or_fetch_repo, KeyPair},
    },
    CONFIGS_DIR, DOCKER, PKG_NAME, REPOS_DIR,
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
}

fn response(status: StatusCode) -> Result<Response<Body>> {
    Ok(Response::builder()
        .status(status)
        .body(Body::empty())
        .unwrap())
}

fn trigger_update(name: String, repo_url: String, tx: mpsc::Sender<Config>) {
    let repo_path = [&REPOS_DIR, &name].iter().collect::<PathBuf>();

    tokio::spawn(async move {
        if let Err(why) = clone_or_fetch_repo(&SSH_KEY, &repo_url, &repo_path) {
            error!(
                "Failed to get repo {} ({} -> {:#?}): {:#?}",
                name, repo_url, repo_path, why
            );
        }

        if repo_path.join("Dockerfile").is_file() {
            trace!("Building image: {}", name);
            if let Err(why) = build_image(&DOCKER, &name, &repo_path).await {
                error!("Failed to build image {}: {:#?}", name, why);
            }

            let config_path = [&CONFIGS_DIR, &name]
                .iter()
                .collect::<PathBuf>()
                .with_extension("toml");
            if config_path.is_file() {
                trace!("Reading config {:#?}", config_path);
                let config = Config::from_file(config_path).await.unwrap();

                if name == PKG_NAME {
                    trace!("Self-update triggered");
                    tx.send(config).await.unwrap();
                } else {
                    let containers = find_containers_with_image(&DOCKER, &name).await;
                    match containers {
                        Ok(conts) => {
                            for c in conts {
                                if let Some(id) = c.id {
                                    trace!("Stopping {} ({})", id, name);
                                    if let Err(why) = stop_container(&DOCKER, &id).await {
                                        error!("Failed to stop container {}: {:#?}", name, why);
                                    }
                                }
                            }

                            trace!("Running {}", name);
                            if let Err(why) = run_container(&DOCKER, config).await {
                                error!("Failed to start container {}: {:#?}", name, why);
                            }
                        }
                        Err(why) => error!(
                            "Failed to list containers with image name {}: {}",
                            name, why
                        ),
                    }
                }
            }
        }
    });
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
                    trace!("Received POST request");

                    let headers = req.headers();
                    let get = move |key| Some(headers.get(key)?.to_str().ok()?.to_string());
                    let headers = get("X-Hub-Signature-256").zip(get("X-GitHub-Event"));
                    if headers.is_none() {
                        trace!("Invalid headers");
                        return response(StatusCode::BAD_REQUEST);
                    }

                    let (git_sig, _event) = headers.unwrap();
                    let buf = body::aggregate(req.into_body()).await;
                    if buf.is_err() {
                        trace!("Failed to aggregate buffer");
                        return response(StatusCode::BAD_REQUEST);
                    }

                    let buf = buf.unwrap();
                    let mut reader = buf.reader();
                    let mut body = String::new();
                    // Fails if body contains invalid UTF-8
                    if reader.read_to_string(&mut body).is_err() {
                        trace!("Invalid UTF-8 in body");
                        return response(StatusCode::BAD_REQUEST);
                    }

                    let sig = HMAC::mac(body.as_bytes(), &SECRET);
                    if git_sig[7..] != hex::encode(sig) {
                        trace!("Invalid signature");
                        return response(StatusCode::UNAUTHORIZED);
                    }

                    info!("Valid signature");
                    let data = json::parse(&body);
                    if data.is_err() {
                        trace!("Failed parse JSON payload");
                        return response(StatusCode::BAD_REQUEST);
                    }

                    let data = data.unwrap();
                    let repo = &data["repository"];
                    let params = repo["name"].as_str().zip(repo["ssh_url"].as_str());
                    if params.is_none() {
                        trace!("Invalid JSON data");
                        return response(StatusCode::BAD_REQUEST);
                    }

                    let (name, repo_url) = params.unwrap();
                    trigger_update(name.to_string(), repo_url.to_string(), tx);

                    trace!("Ok!");
                    response(StatusCode::OK)
                }
                _ => {
                    trace!("Non-POST request discarded: {:#?}", req);
                    response(StatusCode::METHOD_NOT_ALLOWED)
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

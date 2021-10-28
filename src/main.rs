#[macro_use]
extern crate lazy_static;

use anyhow::Result;
use dotenv::dotenv;
use hmac_sha256::HMAC;
use hyper::{
    body::Buf,
    service::{make_service_fn, service_fn},
    Body, Method, Request, Response, Server, StatusCode,
};
use std::{env, io::Read, net::SocketAddr};

lazy_static! {
    static ref SECRET: Vec<u8> = env::var("SECRET_TOKEN")
        .expect("Expected a secret token in the environment")
        .into_bytes();
}

async fn handle(req: Request<Body>) -> Result<Response<Body>> {
    match req.method() {
        &Method::POST => {
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
                let _data = json::parse(&body)?;

                Ok(Response::new(Body::from(body)))
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
    let service = make_service_fn(|_| async { Ok::<_, hyper::Error>(service_fn(handle)) });
    let server = Server::bind(&addr).serve(service);

    if let Err(e) = server.await {
        eprintln!("Server error: {}", e);
    }
}

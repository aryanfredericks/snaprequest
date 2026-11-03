//! A tiny in-memory "shop" API for the example collection in `examples/shop/` and the CI
//! job that runs it.
//!
//! ```sh
//! cargo run -p snapreq-test --example mock_shop -- 8765
//! ```
//!
//! - `GET /health` → `{"status": "ok"}`
//! - `POST /login` (`{"user": "demo", "password": "letmein"}`) → `{"token": "..."}`
//! - `GET /orders` → the orders (bearer token required for every `/orders` route)
//! - `POST /orders` (`{"item": "..."}`) → 201 with the order, state `pending`
//! - `GET /orders/{id}` → the order; it becomes `ready` on the third poll
//! - `DELETE /orders/{id}` → 204, then the order is gone (404)
//!
//! For the web UI's end-to-end tests:
//! - `GET /catalog?items=N` → a JSON array of `N` products (default 20), with a `Set-Cookie`
//! - `GET /page` → a small HTML page
//! - `GET /old-health` → 302 to `/health`
#![allow(clippy::unwrap_used)] // example code

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpListener;

const TOKEN: &str = "demo-token-7f3a9c";

#[derive(Default)]
struct Shop {
    next_id: u64,
    /// id → (order, polls)
    orders: BTreeMap<u64, (Value, u32)>,
}

fn reply(status: u16, body: &Value) -> hyper::Response<Full<Bytes>> {
    let text = if body.is_null() {
        String::new()
    } else {
        body.to_string()
    };
    hyper::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("x-request-id", format!("req-{:x}", std::process::id()))
        .body(Full::new(Bytes::from(text)))
        .unwrap()
}

async fn handle(
    shop: Arc<Mutex<Shop>>,
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    let method = req.method().as_str().to_owned();
    let path = req.uri().path().trim_end_matches('/').to_owned();
    let query = req.uri().query().unwrap_or("").to_owned();
    let authorized = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {TOKEN}"));
    let body = req.into_body().collect().await.unwrap().to_bytes();
    let input: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);

    let resp = match (method.as_str(), path.as_str()) {
        ("GET", "/health") => reply(200, &json!({ "status": "ok" })),
        ("GET", "/catalog") => {
            let n: usize = query
                .split('&')
                .find_map(|p| p.strip_prefix("items="))
                .and_then(|v| v.parse().ok())
                .unwrap_or(20);
            let items: Vec<Value> = (1..=n)
                .map(|i| json!({ "sku": format!("sku-{i}"), "name": format!("Product {i}"), "price": i * 3 % 100 }))
                .collect();
            let mut resp = reply(200, &json!(items));
            resp.headers_mut().insert(
                "set-cookie",
                "visited=yes; Path=/; Max-Age=600; HttpOnly; SameSite=Lax"
                    .parse()
                    .unwrap(),
            );
            resp
        }
        ("GET", "/page") => hyper::Response::builder()
            .header("content-type", "text/html; charset=utf-8")
            .body(Full::new(Bytes::from_static(
                b"<!doctype html><title>Shop</title><h1>Welcome to the shop</h1><p>Preview me.</p>",
            )))
            .unwrap(),
        ("GET", "/old-health") => hyper::Response::builder()
            .status(302)
            .header("location", "/health")
            .body(Full::new(Bytes::new()))
            .unwrap(),
        ("POST", "/login") => {
            if input["user"] == "demo" && input["password"] == "letmein" {
                reply(200, &json!({ "token": TOKEN, "expires_in": 3600 }))
            } else {
                reply(401, &json!({ "error": "invalid_credentials" }))
            }
        }
        (_, p) if p.starts_with("/orders") && !authorized => {
            reply(401, &json!({ "error": "unauthorized" }))
        }
        ("GET", "/orders") => {
            let shop = shop.lock().unwrap();
            let list: Vec<&Value> = shop.orders.values().map(|(o, _)| o).collect();
            reply(200, &json!({ "orders": list }))
        }
        ("POST", "/orders") => match input["item"].as_str() {
            Some(item) if !item.is_empty() => {
                let mut shop = shop.lock().unwrap();
                shop.next_id += 1;
                let id = shop.next_id;
                let order = json!({ "id": id, "item": item, "state": "pending" });
                shop.orders.insert(id, (order.clone(), 0));
                reply(201, &order)
            }
            _ => reply(422, &json!({ "error": "item is required" })),
        },
        (m, p) if p.starts_with("/orders/") => {
            let id: u64 = p["/orders/".len()..].parse().unwrap_or(0);
            let mut shop = shop.lock().unwrap();
            match (m, shop.orders.get_mut(&id)) {
                ("GET", Some((order, polls))) => {
                    *polls += 1;
                    if *polls >= 3 {
                        order["state"] = json!("ready");
                    }
                    reply(200, order)
                }
                ("DELETE", Some(_)) => {
                    shop.orders.remove(&id);
                    reply(204, &Value::Null)
                }
                _ => reply(404, &json!({ "error": "not found" })),
            }
        }
        _ => reply(404, &json!({ "error": "not found" })),
    };
    Ok(resp)
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(8765);
    let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    println!("mock shop listening on http://127.0.0.1:{port}");
    let shop = Arc::new(Mutex::new(Shop::default()));
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let shop = shop.clone();
        tokio::spawn(async move {
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(
                    TokioIo::new(stream),
                    service_fn(move |r| handle(shop.clone(), r)),
                )
                .await;
        });
    }
}

//! AWS SigV4: a mock service re-computes the signature from the request it receives.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use ring::hmac;
use snapreq_core::model::{Auth, Body};
use snapreq_core::{Client, Request};
use tokio::net::TcpListener;

const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(b: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, b).as_ref())
}

fn encode(s: &str, keep_slash: bool) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b'/' if keep_slash => "/".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Verifies the request the way AWS does. Returns why it does not verify.
fn verify(
    method: &str,
    path: &str,
    query: &str,
    headers: &HashMap<String, Vec<String>>,
    body: &[u8],
) -> Result<(), String> {
    let get = |n: &str| headers.get(n).map(|v| v.join(",")).unwrap_or_default();
    let auth = get("authorization");
    let rest = auth.strip_prefix("AWS4-HMAC-SHA256 ").ok_or("not SigV4")?;
    let fields: HashMap<&str, &str> = rest.split(", ").filter_map(|f| f.split_once('=')).collect();
    let cred: Vec<&str> = fields["Credential"].split('/').collect();
    let (access, date, region, service) = (cred[0], cred[1], cred[2], cred[3]);
    if access != "AKIDEXAMPLE" {
        return Err("unknown key".into());
    }
    let signed: Vec<&str> = fields["SignedHeaders"].split(';').collect();
    if !signed.contains(&"host") || !signed.contains(&"x-amz-date") {
        return Err(format!("host/x-amz-date not signed: {signed:?}"));
    }
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            let dec = |s: &str| {
                percent_encoding::percent_decode_str(s)
                    .decode_utf8_lossy()
                    .into_owned()
            };
            (encode(&dec(k), false), encode(&dec(v), false))
        })
        .collect();
    pairs.sort();
    let canonical_query = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let canonical_uri = if service == "s3" {
        path.to_owned()
    } else {
        encode(path, true)
    };
    let canonical_headers: String = signed
        .iter()
        .map(|h| {
            format!(
                "{h}:{}\n",
                get(h).split_whitespace().collect::<Vec<_>>().join(" ")
            )
        })
        .collect();
    let payload = match headers.get("x-amz-content-sha256") {
        Some(v) => v[0].clone(),
        None => sha256(body),
    };
    if payload != "UNSIGNED-PAYLOAD" && payload != sha256(body) {
        return Err("payload hash does not match the body".into());
    }
    let canonical = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{}\n{payload}",
        fields["SignedHeaders"]
    );
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let sts = format!(
        "AWS4-HMAC-SHA256\n{}\n{scope}\n{}",
        get("x-amz-date"),
        sha256(canonical.as_bytes())
    );
    let mut key = format!("AWS4{SECRET}").into_bytes();
    for part in [date, region, service, "aws4_request"] {
        key = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &key), part.as_bytes())
            .as_ref()
            .to_vec();
    }
    let expected =
        hex(hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &key), sts.as_bytes()).as_ref());
    if expected == fields["Signature"] {
        Ok(())
    } else {
        Err(format!(
            "signature mismatch; canonical request:\n{canonical}"
        ))
    }
}

/// `/redirect` sends a 307 to `/target`; everything else is verified and echoes the
/// signed headers.
async fn handle(
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    let method = req.method().to_string();
    let path = req.uri().path().to_owned();
    let query = req.uri().query().unwrap_or_default().to_owned();
    let mut headers: HashMap<String, Vec<String>> = HashMap::new();
    for (n, v) in req.headers() {
        headers
            .entry(n.as_str().to_owned())
            .or_default()
            .push(v.to_str().unwrap().to_owned());
    }
    let body = req.into_body().collect().await.unwrap().to_bytes();
    if path == "/redirect" {
        return Ok(hyper::Response::builder()
            .status(307)
            .header("location", "/target?x=1")
            .body(Full::new(Bytes::new()))
            .unwrap());
    }
    let (status, text) = match verify(&method, &path, &query, &headers, &body) {
        Ok(()) => (
            200,
            format!(
                "ok {}",
                headers
                    .get("authorization")
                    .map(|a| a[0].clone())
                    .unwrap_or_default()
            ),
        ),
        Err(why) => (403, why),
    };
    Ok(hyper::Response::builder()
        .status(status)
        .body(Full::new(Bytes::from(text)))
        .unwrap())
}

async fn start() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(s), service_fn(handle))
                    .await;
            });
        }
    });
    addr
}

fn aws(service: &str, token: &str) -> Auth {
    Auth::AwsSigV4 {
        access_key: "AKIDEXAMPLE".into(),
        secret_key: SECRET.into(),
        session_token: token.into(),
        region: "eu-west-1".into(),
        service: service.into(),
    }
}

#[tokio::test]
async fn signed_requests_verify_on_the_server() {
    let addr = start().await;
    let client = Client::default();

    let get = Request::get(format!("http://{addr}/a%20b/items?tag=x+y"))
        .query("q", "a b/c")
        .query("empty", "")
        .header("X-Custom", "  spaced   out  ")
        .auth(aws("execute-api", ""));
    let resp = client.execute(&get).await.unwrap();
    assert_eq!(resp.status, 200, "{}", resp.text().unwrap());
    // Sent in strict encoding, exactly as signed.
    assert!(
        resp.url
            .ends_with("/a%20b/items?tag=x%20y&q=a%20b%2Fc&empty="),
        "{}",
        resp.url
    );

    let post = Request::new(http::Method::POST, format!("http://{addr}/orders"))
        .body(Body::Json {
            content: r#"{"id":1}"#.into(),
        })
        .auth(aws("execute-api", "temporary-token"));
    let resp = client.execute(&post).await.unwrap();
    assert_eq!(resp.status, 200, "{}", resp.text().unwrap());
    assert!(resp.text().unwrap().contains("x-amz-security-token"));

    // S3: a file body is hashed and the hash header is added.
    let file = std::env::temp_dir().join(format!("snapreq-sigv4-{}.txt", std::process::id()));
    std::fs::write(&file, "x".repeat(200_000)).unwrap();
    let put = Request::new(
        http::Method::PUT,
        format!("http://{addr}/bucket/my%20key.txt"),
    )
    .body(Body::File {
        path: file,
        content_type: None,
    })
    .auth(aws("s3", ""));
    let resp = client.execute(&put).await.unwrap();
    assert_eq!(resp.status, 200, "{}", resp.text().unwrap());
    assert!(resp.text().unwrap().contains("x-amz-content-sha256"));

    // `UNSIGNED-PAYLOAD` set by hand is used as the payload hash.
    let unsigned = Request::new(http::Method::PUT, format!("http://{addr}/bucket/k"))
        .header("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
        .body(Body::Text {
            content: "big".into(),
            content_type: None,
        })
        .auth(aws("s3", ""));
    assert_eq!(client.execute(&unsigned).await.unwrap().status, 200);
}

#[tokio::test]
async fn every_hop_is_signed_and_secrets_stay_hidden() {
    let addr = start().await;
    let resp = Client::default()
        .execute(
            &Request::new(http::Method::PUT, format!("http://{addr}/redirect"))
                .auth(aws("execute-api", "temporary-token")),
        )
        .await
        .unwrap();
    assert_eq!(resp.status, 200, "{}", resp.text().unwrap());
    let trace = serde_json::to_string(&resp.trace).unwrap();
    assert!(
        !trace.contains(SECRET) && !trace.contains("temporary-token"),
        "{trace}"
    );
    assert!(trace.contains("AWS4-HMAC-SHA256 <redacted>"), "{trace}");
}

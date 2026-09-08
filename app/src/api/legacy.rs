use axum::{
    body::Body,
    http::{HeaderMap, StatusCode},
    response::Response,
};

fn json_ok(body: &str) -> Response<Body> {
    Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

pub async fn resource() -> Response<Body> {
    json_ok(r#"{"result":"ok"}"#) // Skip client resource
}

pub async fn xenia_servers(headers: HeaderMap) -> Response<Body> {
    if headers.get("user-agent").and_then(|h| h.to_str().ok()) != Some("xenia") {
        return Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap();
    }
    json_ok(r#"[{"address":"127.0.0.1","flags":0,"description":"Sentient.A"}]"#)
}

pub async fn xenia_ports(headers: HeaderMap) -> Response<Body> {
    if headers.get("user-agent").and_then(|h| h.to_str().ok()) != Some("xenia") {
        return Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap();
    }
    json_ok(r#"{"connect":[{"info":"Server","port":80,"mappedTo":8085}]}"#)
}

pub async fn handle_sentient() -> Response<Body> {
    let s = b"IDK";
    let mut body = Vec::with_capacity(4 + s.len());
    body.extend_from_slice(&(s.len() as u32).to_be_bytes());
    body.extend_from_slice(s);

    Response::builder()
        .status(200)
        .header("Content-Type", "bin/xtsw")
        .body(Body::from(body))
        .unwrap()
}
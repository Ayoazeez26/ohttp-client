//! The OHTTP gateway: serves its key config and relay opt-in, and
//! decapsulates requests, forwarding them to the inner target.

use std::io::Cursor;

use super::server::HttpRequest;
use super::GATEWAY_PATH;

/// Gateway opt-in advertised to the relay's prober: ALPN-style list
/// containing the one purpose `ohttp_relay` requires before it forwards.
const ALLOWED_PURPOSES_BODY: &[u8] = b"\x00\x01\x2aTEST 454403bb-9f7b-4385-b31f-acd2dae20b7e";

pub(super) fn handle_gateway_request(
    server: &ohttp::Server,
    req: HttpRequest,
) -> (u16, Vec<(String, String)>, Vec<u8>) {
    match (req.method.as_str(), req.path.split('?').next().unwrap()) {
        ("GET", GATEWAY_PATH) if req.path.contains("allowed_purposes") => (
            200,
            vec![(
                "content-type".into(),
                "application/x-ohttp-allowed-purposes".into(),
            )],
            ALLOWED_PURPOSES_BODY.to_vec(),
        ),
        ("GET", GATEWAY_PATH) => (
            200,
            vec![("content-type".into(), "application/ohttp-keys".into())],
            ohttp::KeyConfig::encode_list(&[server.config()]).unwrap(),
        ),
        ("POST", GATEWAY_PATH) => {
            assert_eq!(req.header("content-type"), Some("message/ohttp-req"));
            let (bhttp_bytes, response_ctx) = server.decapsulate(&req.body).unwrap();
            let inner = bhttp::Message::read_bhttp(&mut Cursor::new(&bhttp_bytes[..])).unwrap();
            let inner_response = forward_to_target(&inner);

            let mut bhttp_res = Vec::new();
            inner_response
                .write_bhttp(bhttp::Mode::KnownLength, &mut bhttp_res)
                .unwrap();
            (
                200,
                vec![("content-type".into(), "message/ohttp-res".into())],
                response_ctx.encapsulate(&bhttp_res).unwrap(),
            )
        }
        _ => (404, vec![], b"Not Found".to_vec()),
    }
}

/// Send the decapsulated inner request to the authority it names and build
/// the inner BHTTP response from what comes back.
fn forward_to_target(inner: &bhttp::Message) -> bhttp::Message {
    let part = |v: Option<&[u8]>| String::from_utf8(v.unwrap().to_vec()).unwrap();
    let method = part(inner.control().method());
    let url = format!(
        "{}://{}{}",
        part(inner.control().scheme()),
        part(inner.control().authority()),
        part(inner.control().path()),
    );

    let mut forward = bitreq::Request::new(bitreq::Method::Custom(method), url);
    for field in inner.header().iter() {
        forward = forward.with_header(
            String::from_utf8_lossy(field.name()).into_owned(),
            String::from_utf8_lossy(field.value()).into_owned(),
        );
    }
    let res = forward.with_body(inner.content().to_vec()).send().unwrap();

    let status = bhttp::StatusCode::try_from(u16::try_from(res.status_code).unwrap()).unwrap();
    let mut message = bhttp::Message::response(status);
    for (name, value) in &res.headers {
        message.put_header(name.as_str(), value.as_str());
    }
    message.write_content(res.as_bytes());
    message
}

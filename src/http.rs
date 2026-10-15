//! `bitreq` integration: key config fetch and a request builder that sends
//! through the relay.

use crate::{parse_key_config, Error, KeyConfig, OhttpClient, Response, Url};

/// GET the gateway's key endpoint directly. This reveals the caller's IP to
/// the gateway; [`fetch_key_config_via_relay`] does not.
pub async fn fetch_key_config(gateway_key_url: &str) -> Result<KeyConfig, Error> {
    let res = bitreq::get(gateway_key_url).send_async().await?;
    if res.status_code != 200 {
        return Err(Error::UnexpectedStatus(res.status_code));
    }
    parse_key_config(res.as_bytes())
}

/// GET the gateway's key endpoint through the relay with HTTP `CONNECT`, so
/// the gateway only sees the relay's IP.
pub async fn fetch_key_config_via_relay(
    gateway_key_url: &str,
    relay_url: &Url,
) -> Result<KeyConfig, Error> {
    let host = relay_url.host_str().ok_or(Error::NoRelayHost)?;
    let port = relay_url
        .port_or_known_default()
        .ok_or(Error::NoRelayHost)?;
    let proxy = bitreq::Proxy::new_http(format!("{host}:{port}"))?;
    let res = bitreq::get(gateway_key_url)
        .with_proxy(proxy)
        .send_async()
        .await?;
    if res.status_code != 200 {
        return Err(Error::UnexpectedStatus(res.status_code));
    }
    parse_key_config(res.as_bytes())
}

impl OhttpClient {
    /// Fetch the key config with [`fetch_key_config_via_relay`] and build a
    /// client. See [`Self::new`] for `target`.
    pub async fn from_gateway(
        relay: Url,
        target: Url,
        gateway_key_url: &str,
    ) -> Result<Self, Error> {
        let key_config = fetch_key_config_via_relay(gateway_key_url, &relay).await?;
        Ok(Self::new(relay, target, key_config))
    }

    /// Start a request to `path` on the target origin. See
    /// [`Self::encapsulate`] for how `path` resolves.
    pub fn request(
        &self,
        method: impl Into<String>,
        path: impl Into<String>,
    ) -> RequestBuilder<'_> {
        RequestBuilder {
            client: self,
            method: method.into(),
            path: path.into(),
            headers: Vec::new(),
            params: Vec::new(),
            body: None,
        }
    }

    pub fn get(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("GET", path)
    }

    pub fn head(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("HEAD", path)
    }

    pub fn post(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("POST", path)
    }

    pub fn put(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("PUT", path)
    }

    pub fn delete(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("DELETE", path)
    }

    pub fn connect(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("CONNECT", path)
    }

    pub fn options(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("OPTIONS", path)
    }

    pub fn trace(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("TRACE", path)
    }

    pub fn patch(&self, path: impl Into<String>) -> RequestBuilder<'_> {
        self.request("PATCH", path)
    }
}

/// Inner request sent through the relay with `bitreq`. Created by
/// [`OhttpClient::request`].
#[must_use = "call `send()` to perform the request"]
pub struct RequestBuilder<'a> {
    client: &'a OhttpClient,
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    params: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

impl RequestBuilder<'_> {
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Add a query parameter, percent-encoded on send.
    pub fn param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.push((key.into(), value.into()));
        self
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Encapsulate, POST to the relay, and decapsulate the response. Fails
    /// with [`Error::UnexpectedStatus`] unless the relay returns 200.
    pub async fn send(self) -> Result<Response, Error> {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let query: Vec<(&str, &str)> = self
            .params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let (req, ctx) = self.client.encapsulate(
            &self.method,
            &self.path,
            &headers,
            &query,
            self.body.as_deref(),
        )?;
        let res = bitreq::post(req.url.as_str())
            .with_header("content-type", req.content_type)
            .with_body(req.body)
            .send_async()
            .await?;
        // TODO: other status codes?
        if res.status_code != 200 {
            return Err(Error::UnexpectedStatus(res.status_code));
        }
        ctx.decapsulate(res.as_bytes())
    }
}

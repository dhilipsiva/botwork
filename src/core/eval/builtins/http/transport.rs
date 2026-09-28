use super::*;
use config::{Plan, Redirects};
use http_body_util::{BodyExt, Full};
use hyper::body::Body;
use hyper::{
    header::{CONNECTION, HOST, LOCATION},
    HeaderMap, Method, Request,
};
use hyper_util::rt::TokioIo;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use tokio::io::{AsyncRead, AsyncWrite};
use url::{Host, Position};

pub(super) struct Response {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    pub url: String,
    pub redirects: usize,
    pub attempts: usize,
}

pub(super) async fn request(
    context: &mut Context,
    mut plan: Plan,
    retention: Arc<Retention>,
    control: &OperationControl,
) -> EvaluationResult<Response> {
    let mut redirects = 0;
    let mut retries = 0;
    let mut attempts = 0;
    let tls = tls_config(context, &plan)?;
    loop {
        context.checkpoint()?;
        control
            .checkpoint()
            .map_err(|error| stopped(context, error))?;
        attempts += 1;
        let (status, headers, body) = once(context, &plan, retention.clone(), tls.clone()).await?;
        if plan.redirects != Redirects::None && matches!(status, 301 | 302 | 303 | 307 | 308) {
            if let Some(location) = headers.get(LOCATION) {
                if redirects >= plan.max_redirects {
                    return Err(failed(context, "HTTP redirect limit exceeded"));
                }
                if headers.get_all(LOCATION).iter().count() != 1 {
                    return Err(failed(
                        context,
                        "HTTP redirect has multiple Location headers",
                    ));
                }
                let location = location
                    .to_str()
                    .map_err(|_| failed(context, "HTTP redirect Location is not ASCII"))?;
                if location.len() > MAX_URL {
                    return Err(limit(context, "HTTP URL bytes", MAX_URL));
                }
                if location
                    .bytes()
                    .any(|b| b <= 0x20 || b == 0x7f || b == b'\\')
                {
                    return Err(failed(
                        context,
                        "HTTP redirect Location contains forbidden characters",
                    ));
                }
                let joined = plan
                    .url
                    .join(location)
                    .map_err(|_| failed(context, "Invalid HTTP redirect Location"))?;
                let next = config::parse_url(context, joined.as_str())?;
                if plan.url.scheme() == "https" && next.scheme() != "https" {
                    return Err(failed(context, "HTTP redirects cannot downgrade HTTPS"));
                }
                let cross = plan.url.origin() != next.origin();
                if cross && plan.redirects == Redirects::SameOrigin {
                    return Err(failed(context, "HTTP redirect crosses origins"));
                }
                if (status == 303 && plan.method != Method::HEAD)
                    || (matches!(status, 301 | 302) && plan.method == Method::POST)
                {
                    plan.method = Method::GET;
                    plan.body = bytes::Bytes::new();
                    let names: Vec<_> = plan
                        .headers
                        .keys()
                        .filter(|name| {
                            name.as_str().starts_with("content-")
                                || matches!(name.as_str(), "digest" | "expect")
                        })
                        .cloned()
                        .collect();
                    for name in names {
                        plan.headers.remove(name);
                    }
                }
                if cross {
                    if !matches!(plan.method, Method::GET | Method::HEAD) || !plan.body.is_empty() {
                        return Err(failed(
                            context,
                            "Cross-origin HTTP redirects require GET or HEAD with an empty body",
                        ));
                    }
                    // Arbitrary user headers can contain credentials; drop all of them.
                    plan.headers.clear();
                }
                plan.url = next;
                redirects += 1;
                continue;
            }
        }
        if retries < plan.max_retries && matches!(status, 429 | 502 | 503 | 504) {
            retries += 1;
            tokio::time::sleep(Duration::from_millis(plan.retry_delay_ms)).await;
            continue;
        }
        return Ok(Response {
            status,
            headers,
            body,
            url: plan.url.to_string(),
            redirects,
            attempts,
        });
    }
}

fn tls_config(context: &Context, plan: &Plan) -> EvaluationResult<Arc<rustls::ClientConfig>> {
    let mut roots =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(certs) = &plan.ca {
        for cert in certs {
            roots
                .add(cert.clone())
                .map_err(|_| invalid(context, "HTTP ca_pem contains an invalid trust anchor"))?;
        }
    }
    let builder = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| failed(context, "HTTP TLS configuration failed"))?;
    let mut config = builder.with_root_certificates(roots).with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

pub(super) struct PendingDns(
    pub(super) tokio::task::JoinHandle<(std::io::Result<Vec<SocketAddr>>, Arc<Retention>)>,
);
impl Drop for PendingDns {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn addresses(
    context: &mut Context,
    plan: &Plan,
    retention: Arc<Retention>,
) -> EvaluationResult<Vec<SocketAddr>> {
    let port = plan.url.port_or_known_default().expect("HTTP scheme");
    let host = match plan.url.host().expect("validated host") {
        Host::Ipv4(ip) => return Ok(vec![SocketAddr::new(IpAddr::V4(ip), port)]),
        Host::Ipv6(ip) => return Ok(vec![SocketAddr::new(IpAddr::V6(ip), port)]),
        Host::Domain(host) => host.to_owned(),
    };
    // Started OS lookups cannot be forcibly stopped. Their slot and byte leases
    // remain owned by this worker (including an undelivered result) until done.
    let mut pending = PendingDns(tokio::task::spawn_blocking(move || {
        let result = (host.as_str(), port)
            .to_socket_addrs()
            .map(|addresses| addresses.take(65).collect());
        (result, retention)
    }));
    let (result, _retained) = (&mut pending.0)
        .await
        .map_err(|_| failed(context, "HTTP DNS worker failed"))?;
    let result = result.map_err(|_| failed(context, "HTTP DNS resolution failed"))?;
    if result.len() > 64 {
        return Err(limit(context, "HTTP DNS addresses", 64));
    }
    if result.is_empty() {
        return Err(failed(context, "HTTP DNS returned no addresses"));
    }
    Ok(result)
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}
async fn once(
    context: &mut Context,
    plan: &Plan,
    retention: Arc<Retention>,
    tls: Arc<rustls::ClientConfig>,
) -> EvaluationResult<(u16, HeaderMap, Vec<u8>)> {
    let addresses = addresses(context, plan, retention).await?;
    let mut stream = None;
    for address in addresses {
        if let Ok(connected) = tokio::net::TcpStream::connect(address).await {
            stream = Some(connected);
            break;
        }
    }
    let stream = stream.ok_or_else(|| failed(context, "HTTP TCP connection failed"))?;
    let stream: Box<dyn Stream> = if plan.url.scheme() == "https" {
        let name = match plan.url.host().expect("validated host") {
            Host::Ipv4(ip) => rustls::pki_types::ServerName::IpAddress(IpAddr::V4(ip).into()),
            Host::Ipv6(ip) => rustls::pki_types::ServerName::IpAddress(IpAddr::V6(ip).into()),
            Host::Domain(name) => rustls::pki_types::ServerName::try_from(name.to_owned())
                .map_err(|_| invalid(context, "Invalid HTTP TLS server name"))?,
        };
        Box::new(
            tokio_rustls::TlsConnector::from(tls)
                .connect(name, stream)
                .await
                .map_err(|_| failed(context, "HTTP TLS verification or handshake failed"))?,
        )
    } else {
        Box::new(stream)
    };
    let mut builder = hyper::client::conn::http1::Builder::new();
    builder.max_headers(plan.max_headers).max_buf_size(65_536);
    let (mut sender, connection) = builder
        .handshake(TokioIo::new(stream))
        .await
        .map_err(|_| failed(context, "HTTP connection handshake failed"))?;
    tokio::pin!(connection);
    let mut request = Request::builder()
        .method(plan.method.clone())
        .uri(&plan.url[Position::BeforePath..Position::AfterQuery])
        .body(Full::new(plan.body.clone()))
        .map_err(|_| invalid(context, "Invalid HTTP request target"))?;
    *request.headers_mut() = plan.headers.clone();
    let authority = &plan.url[Position::BeforeHost..Position::AfterPort];
    request.headers_mut().insert(
        HOST,
        authority
            .parse()
            .map_err(|_| invalid(context, "Invalid HTTP host"))?,
    );
    request
        .headers_mut()
        .insert(CONNECTION, hyper::header::HeaderValue::from_static("close"));
    let exchange = async move {
        let response = sender
            .send_request(request)
            .await
            .map_err(|error| protocol_error(context, error))?;
        let status = response.status().as_u16();
        if status == 101 {
            return Err(failed(context, "HTTP protocol upgrades are unsupported"));
        }
        let (parts, mut body) = response.into_parts();
        let mut headers = parts.headers;
        let mut header_bytes = check_headers(context, plan, &headers)?;
        let mut count = headers.len();
        // A HEAD Content-Length describes the resource, not response bytes.
        if plan.method != Method::HEAD && body.size_hint().lower() > plan.max_body as u64 {
            return Err(limit(context, "HTTP response body bytes", plan.max_body));
        }
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            context.checkpoint()?;
            let frame = frame.map_err(|error| protocol_error(context, error))?;
            if let Some(data) = frame.data_ref() {
                if data.len() > plan.max_body.saturating_sub(bytes.len()) {
                    return Err(limit(context, "HTTP response body bytes", plan.max_body));
                }
                bytes.extend_from_slice(data);
            }
            if let Some(trailers) = frame.trailers_ref() {
                header_bytes += check_headers(context, plan, trailers)?;
                count += trailers.len();
                if header_bytes > plan.max_header_bytes || count > plan.max_headers {
                    return Err(limit(
                        context,
                        "HTTP response headers",
                        plan.max_header_bytes,
                    ));
                }
                for (name, value) in trailers {
                    headers.append(name.clone(), value.clone());
                }
            }
        }
        if !plan.binary
            && (std::str::from_utf8(&bytes).is_err()
                || headers
                    .values()
                    .any(|value| std::str::from_utf8(value.as_bytes()).is_err()))
        {
            return Err(invalid(context, "HTTP response is not valid UTF-8"));
        }
        Ok((status, headers, bytes))
    };
    tokio::pin!(exchange);
    // The transport is polled in this future, never detached into a task.
    // Dropping/cancelling the enclosing request immediately drops the socket.
    tokio::select! {
        result = &mut exchange => result,
        _ = &mut connection => exchange.await,
    }
}
fn check_headers(context: &Context, plan: &Plan, headers: &HeaderMap) -> EvaluationResult<usize> {
    let bytes: usize = headers
        .iter()
        .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
        .sum();
    if headers.len() > plan.max_headers
        || bytes > plan.max_header_bytes
        || headers.keys().any(|name| name.as_str().len() > 256)
    {
        return Err(limit(
            context,
            "HTTP response headers",
            plan.max_header_bytes,
        ));
    }
    Ok(bytes)
}
fn protocol_error(context: &Context, error: hyper::Error) -> RuntimeDiagnostic {
    if error.is_parse_too_large() {
        limit(context, "HTTP response header parser", 65_536)
    } else {
        failed(context, "HTTP response framing or transport failed")
    }
}

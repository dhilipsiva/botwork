//! Downloading a url source's archive: HTTPS (or plain HTTP to this machine),
//! a few redirects, and a size cap. The archive's SHA-256, not the transport,
//! is what pins its content.
use super::*;
use http_body_util::{BodyExt, Empty};
use hyper::{body::Bytes, Request};
use hyper_util::rt::TokioIo;
use std::{net::IpAddr, sync::Arc, time::Duration};
use url::{Host, Position, Url};

const MAX_REDIRECTS: usize = 5;
const TIMEOUT: Duration = Duration::from_secs(300);

trait Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Stream for T {}

/// Whether a URL may be fetched: HTTPS anywhere, HTTP only on this machine.
fn allowed(url: &Url) -> bool {
    let loopback = match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    url.scheme() == "https" || (url.scheme() == "http" && loopback)
}

/// The body at `url`, at most `MAX_TREE_BYTES`, after up to five redirects.
pub fn get(url: &str) -> Result<Vec<u8>, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("starting the downloader: {error}"))?;
    runtime.block_on(async {
        tokio::time::timeout(TIMEOUT, follow(url))
            .await
            .map_err(|_| format!("{url}: no response within {} seconds", TIMEOUT.as_secs()))?
    })
}

async fn follow(start: &str) -> Result<Vec<u8>, String> {
    let mut url = Url::parse(start).map_err(|error| format!("{start}: {error}"))?;
    for _ in 0..=MAX_REDIRECTS {
        if !allowed(&url) {
            return Err(format!(
                "{url}: use `https`, or `http` only on this machine"
            ));
        }
        match once(&url).await? {
            Ok(body) => return Ok(body),
            Err(location) => {
                url = url
                    .join(&location)
                    .map_err(|error| format!("{url}: redirect to `{location}`: {error}"))?;
            }
        }
    }
    Err(format!("{start}: more than {MAX_REDIRECTS} redirects"))
}

fn tls() -> Result<Arc<rustls::ClientConfig>, String> {
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| format!("TLS configuration: {error}"))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::new(config))
}

/// One request: Ok with the body, or Err(Ok) with a redirect's location.
async fn once(url: &Url) -> Result<Result<Vec<u8>, String>, String> {
    let host = url.host().ok_or_else(|| format!("{url}: no host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| format!("{url}: no port"))?;
    let stream = match host {
        Host::Ipv4(address) => tokio::net::TcpStream::connect((IpAddr::V4(address), port)).await,
        Host::Ipv6(address) => tokio::net::TcpStream::connect((IpAddr::V6(address), port)).await,
        Host::Domain(domain) => tokio::net::TcpStream::connect((domain, port)).await,
    }
    .map_err(|error| format!("{url}: {error}"))?;
    let stream: Box<dyn Stream> = if url.scheme() == "https" {
        let name = match host {
            Host::Ipv4(address) => {
                rustls::pki_types::ServerName::IpAddress(IpAddr::V4(address).into())
            }
            Host::Ipv6(address) => {
                rustls::pki_types::ServerName::IpAddress(IpAddr::V6(address).into())
            }
            Host::Domain(domain) => rustls::pki_types::ServerName::try_from(domain.to_owned())
                .map_err(|_| format!("{url}: not a TLS server name"))?,
        };
        Box::new(
            tokio_rustls::TlsConnector::from(tls()?)
                .connect(name, stream)
                .await
                .map_err(|error| format!("{url}: TLS: {error}"))?,
        )
    } else {
        Box::new(stream)
    };
    let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
        .handshake(TokioIo::new(stream))
        .await
        .map_err(|error| format!("{url}: {error}"))?;
    tokio::spawn(connection);
    let request = Request::get(&url[Position::BeforePath..Position::AfterQuery])
        .header(
            hyper::header::HOST,
            &url[Position::BeforeHost..Position::AfterPort],
        )
        .header(
            hyper::header::USER_AGENT,
            concat!("botwork/", env!("CARGO_PKG_VERSION")),
        )
        .header(hyper::header::CONNECTION, "close")
        .body(Empty::<Bytes>::new())
        .map_err(|error| format!("{url}: {error}"))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|error| format!("{url}: {error}"))?;
    let status = response.status();
    if status.is_redirection() {
        let location = response
            .headers()
            .get(hyper::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| format!("{url}: HTTP {status} without a location"))?;
        return Ok(Err(location.to_owned()));
    }
    if !status.is_success() {
        return Err(format!("{url}: HTTP {status}"));
    }
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| format!("{url}: {error}"))?;
        if let Some(data) = frame.data_ref() {
            if bytes.len() as u64 + data.len() as u64 > MAX_TREE_BYTES {
                return Err(format!("{url}: larger than {MAX_TREE_BYTES} bytes"));
            }
            bytes.extend_from_slice(data);
        }
    }
    Ok(Ok(bytes))
}

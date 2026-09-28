use super::*;
use bytes::Bytes;
use hyper::{
    header::{HeaderName, HeaderValue},
    HeaderMap, Method,
};
use url::Url;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Redirects {
    None,
    SameOrigin,
    AnyOrigin,
}
pub(super) struct Plan {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub binary: bool,
    pub timeout_ms: u64,
    pub max_body: usize,
    pub max_headers: usize,
    pub max_header_bytes: usize,
    pub redirects: Redirects,
    pub max_redirects: usize,
    pub max_retries: usize,
    pub retry_delay_ms: u64,
    pub ca: Option<Vec<rustls::pki_types::CertificateDer<'static>>>,
}
impl Plan {
    pub(super) fn new(
        context: &Context,
        binary: bool,
        method: &Literal,
        url: &Literal,
        options: Option<&Literal>,
    ) -> EvaluationResult<Self> {
        let Literal::String(method) = method else {
            unreachable!("typed method")
        };
        let Literal::String(url) = url else {
            unreachable!("typed URL")
        };
        if method.len() > 64 {
            return Err(invalid(context, "HTTP method exceeds 64 bytes"));
        }
        let method = Method::from_bytes(method.as_bytes())
            .map_err(|_| invalid(context, "HTTP method must be an HTTP token"))?;
        if method == Method::CONNECT {
            return Err(invalid(context, "HTTP CONNECT tunnels are unsupported"));
        }
        let url = parse_url(context, url)?;
        let empty = HashMap::new();
        let options = match options {
            Some(Literal::Map(options)) => options,
            None => &empty,
            _ => unreachable!("typed options"),
        };
        for name in options.keys() {
            if !matches!(
                name.as_str(),
                "headers"
                    | "body"
                    | "timeout_ms"
                    | "max_body_bytes"
                    | "max_headers"
                    | "max_header_bytes"
                    | "redirects"
                    | "max_redirects"
                    | "max_retries"
                    | "retry_delay_ms"
                    | "ca_pem"
            ) {
                return Err(invalid(context, "Unknown HTTP option"));
            }
        }
        let number = |name, default, maximum| -> EvaluationResult<usize> {
            match options.get(name) {
                None => Ok(default),
                Some(Literal::Int(value)) if *value >= 0 && (*value as usize) <= maximum => {
                    Ok(*value as usize)
                }
                _ => Err(context.formatted_error(
                    BWErr::OperationIncompatibleError,
                    format_args!("HTTP {name} must be an Int from 0 to {maximum}"),
                    None,
                    false,
                )),
            }
        };
        let timeout_ms = number("timeout_ms", 30_000, 86_400_000)? as u64;
        let max_body = number(
            "max_body_bytes",
            if binary { 16_384 } else { MAX_BODY },
            MAX_BODY,
        )?;
        let max_headers = number("max_headers", 128, 128)?;
        let max_header_bytes = number("max_header_bytes", 16_384, 65_536)?;
        let max_redirects = number("max_redirects", 10, 10)?;
        let max_retries = number("max_retries", 0, 5)?;
        let retry_delay_ms = number("retry_delay_ms", 100, 60_000)? as u64;
        let redirects = match options.get("redirects") {
            None => Redirects::None,
            Some(Literal::String(value)) => match value.as_str() {
                "none" => Redirects::None,
                "same-origin" => Redirects::SameOrigin,
                "any-origin" => Redirects::AnyOrigin,
                _ => {
                    return Err(invalid(
                        context,
                        "HTTP redirects must be none, same-origin or any-origin",
                    ))
                }
            },
            _ => return Err(invalid(context, "HTTP redirects must be a String")),
        };
        let body = match options.get("body") {
            None => Bytes::new(),
            Some(Literal::String(value)) if !binary => {
                if value.len() > MAX_BODY {
                    return Err(limit(context, "HTTP request body bytes", MAX_BODY));
                }
                Bytes::copy_from_slice(value.as_bytes())
            }
            Some(Literal::Array(values)) if binary => {
                if values.len() > MAX_BODY {
                    return Err(limit(context, "HTTP request body bytes", MAX_BODY));
                }
                let mut body = Vec::with_capacity(values.len());
                for value in values {
                    match value {
                        Literal::Int(byte) if (0..=255).contains(byte) => body.push(*byte as u8),
                        _ => {
                            return Err(invalid(
                                context,
                                "HTTP binary body requires Int bytes from 0 to 255",
                            ))
                        }
                    }
                }
                Bytes::from(body)
            }
            _ => {
                return Err(invalid(
                    context,
                    "HTTP body must be a String for text calls or byte Array for binary calls",
                ))
            }
        };
        if max_retries > 0 && (!matches!(method, Method::GET | Method::HEAD) || !body.is_empty()) {
            return Err(invalid(
                context,
                "HTTP retries require GET or HEAD with an empty body",
            ));
        }
        let mut headers = HeaderMap::new();
        let mut header_bytes = 0;
        if let Some(value) = options.get("headers") {
            let Literal::Map(values) = value else {
                return Err(invalid(context, "HTTP headers must be a Map"));
            };
            for (name, value) in values {
                if name.len() > 256 {
                    return Err(invalid(context, "HTTP header name exceeds 256 bytes"));
                }
                let name = HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| invalid(context, "Invalid HTTP header name"))?;
                if matches!(
                    name.as_str(),
                    "host"
                        | "content-length"
                        | "transfer-encoding"
                        | "connection"
                        | "trailer"
                        | "upgrade"
                        | "proxy-authorization"
                        | "proxy-connection"
                        | "te"
                ) {
                    return Err(invalid(
                        context,
                        "HTTP transport owns framing and connection headers",
                    ));
                }
                let mut append = |value: &Literal| -> EvaluationResult<()> {
                    let Literal::String(value) = value else {
                        return Err(invalid(
                            context,
                            "HTTP header values must be Strings or Arrays of Strings",
                        ));
                    };
                    header_bytes += name.as_str().len() + value.len();
                    if header_bytes > 16_384 || headers.len() >= 128 {
                        return Err(limit(context, "HTTP request headers", 16_384));
                    }
                    let value = HeaderValue::from_bytes(value.as_bytes())
                        .map_err(|_| invalid(context, "Invalid HTTP header value"))?;
                    headers.append(name.clone(), value);
                    Ok(())
                };
                match value {
                    Literal::Array(values) => {
                        for value in values {
                            append(value)?;
                        }
                    }
                    _ => append(value)?,
                }
            }
        }
        let ca = if let Some(value) = options.get("ca_pem") {
            let Literal::String(pem) = value else {
                return Err(invalid(context, "HTTP ca_pem must be a String"));
            };
            if pem.len() > 65_536 {
                return Err(limit(context, "HTTP CA bytes", 65_536));
            }
            use rustls::pki_types::pem::PemObject;
            let certs = rustls::pki_types::CertificateDer::pem_slice_iter(pem.as_bytes())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| invalid(context, "HTTP ca_pem contains invalid certificates"))?;
            if certs.is_empty() {
                return Err(invalid(
                    context,
                    "HTTP ca_pem requires at least one certificate",
                ));
            }
            // Validate trust anchors before the first DNS lookup or connection.
            let mut roots = rustls::RootCertStore::empty();
            for certificate in &certs {
                roots.add(certificate.clone()).map_err(|_| {
                    invalid(context, "HTTP ca_pem contains an invalid trust anchor")
                })?;
            }
            Some(certs)
        } else {
            None
        };
        Ok(Self {
            method,
            url,
            headers,
            body,
            binary,
            timeout_ms,
            max_body,
            max_headers,
            max_header_bytes,
            redirects,
            max_redirects,
            max_retries,
            retry_delay_ms,
            ca,
        })
    }
}
pub(super) fn parse_url(context: &Context, text: &str) -> EvaluationResult<Url> {
    if text.len() > MAX_URL {
        return Err(limit(context, "HTTP URL bytes", MAX_URL));
    }
    if text.bytes().any(|b| b <= 0x20 || b == 0x7f || b == b'\\') {
        return Err(invalid(
            context,
            "HTTP URL contains whitespace, controls or backslashes",
        ));
    }
    let mut url = Url::parse(text).map_err(|_| invalid(context, "Invalid absolute HTTP URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid(
            context,
            "HTTP URL requires http(s), a host and no credentials",
        ));
    }
    url.set_fragment(None);
    if url.as_str().len() > MAX_URL {
        return Err(limit(context, "HTTP URL bytes", MAX_URL));
    }
    Ok(url)
}

//! Host and Origin checks (DNS rebinding, CSRF), token auth, the login rate limit and the
//! response headers (SPEC 14, 20; design 7.5).

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Extension, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::error::{ApiError, ApiResult};
use crate::extract::ApiJson;
use crate::state::{AppState, WebOptions};
use crate::types::TokenRequest;

pub const COOKIE_NAME: &str = "so_token";
/// Cookie lifetime: 7 days.
pub const COOKIE_MAX_AGE: u64 = 7 * 24 * 3600;
/// Failed logins allowed per IP and window.
pub const MAX_FAILURES: usize = 5;
pub const WINDOW: Duration = Duration::from_secs(60);
const MAX_TRACKED_IPS: usize = 1024;

// Sonner 2.0.8 appends an empty style element, then injects one fixed stylesheet.
// Only those two content hashes are allowed and checked by web tests;
// runtime chart/Radix styles receive a fresh nonce for each HTML response.
pub const CSP: &str = "default-src 'self'; style-src 'self'; style-src-attr 'unsafe-inline'; style-src-elem 'self' 'sha256-StEaX+se6YS7pqjzrzMIA0KaX9zF/8zAhvQXZAe5epY=' 'sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU='; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

const TOKEN_PATHS: [&str; 2] = ["/api/v1/token", "/api/v1/auth"];

/// The peer address of a request; loopback when the server is driven without a socket
/// (tests).
#[derive(Debug, Clone, Copy)]
pub struct ClientIp(pub IpAddr);

/// Failed-login counter per IP.
#[derive(Default)]
pub struct RateLimiter {
    failures: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
}

impl RateLimiter {
    /// `Err(seconds to wait)` when `ip` has used up its failures in the window.
    pub fn check(&self, ip: IpAddr) -> Result<(), u64> {
        let mut map = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let Some(q) = map.get_mut(&ip) else {
            return Ok(());
        };
        while q.front().is_some_and(|t| now.duration_since(*t) >= WINDOW) {
            q.pop_front();
        }
        if q.len() < MAX_FAILURES {
            return Ok(());
        }
        let oldest = q.front().copied().unwrap_or(now);
        Err((WINDOW.saturating_sub(now.duration_since(oldest)))
            .as_secs()
            .max(1))
    }

    pub fn record_failure(&self, ip: IpAddr) {
        let mut map = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= MAX_TRACKED_IPS && !map.contains_key(&ip) {
            let now = Instant::now();
            map.retain(|_, q| q.back().is_some_and(|t| now.duration_since(*t) < WINDOW));
            if map.len() >= MAX_TRACKED_IPS
                && let Some(oldest) = map
                    .iter()
                    .filter_map(|(ip, q)| q.back().map(|t| (*ip, *t)))
                    .min_by_key(|(_, at)| *at)
                    .map(|(ip, _)| ip)
            {
                map.remove(&oldest);
            }
        }
        map.entry(ip).or_default().push_back(Instant::now());
    }
}

/// The cookie value for a token: a digest, so the token itself never sits in the browser.
pub fn cookie_value(token: &str) -> String {
    let digest = Sha256::digest(format!("superorchestrator-ui-session:{token}").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// A valid session cookie or `Authorization: Bearer <token>`.
fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let by_cookie = cookie(headers, COOKIE_NAME).is_some_and(|c| ct_eq(c, &cookie_value(token)));
    let by_bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| ct_eq(t, token));
    by_cookie || by_bearer
}

/// Splits `host[:port]`, handling `[v6]:port`. Host is lowercased without brackets.
fn split_authority(authority: &str) -> Option<(String, Option<u16>)> {
    let a = authority.trim();
    if a.is_empty() || a.contains(['/', '@', ' ']) {
        return None;
    }
    let (host, port) = if let Some(rest) = a.strip_prefix('[') {
        let (h, tail) = rest.split_once(']')?;
        (h, tail.strip_prefix(':'))
    } else if a.matches(':').count() == 1 {
        let (h, p) = a.split_once(':')?;
        (h, Some(p))
    } else if a.contains(':') {
        return None;
    } else {
        (a, None)
    };
    let port = match port {
        Some(p) => Some(p.parse::<u16>().ok()?),
        None => None,
    };
    Some((host.to_ascii_lowercase(), port))
}

/// DNS-rebinding defence: the Host must name this machine and this server's port.
pub fn host_allowed(authority: &str, opts: &WebOptions) -> bool {
    let Some((host, port)) = split_authority(authority) else {
        return false;
    };
    if port.is_some_and(|p| opts.port != 0 && p != opts.port) {
        return false;
    }
    if ["localhost", "127.0.0.1", "::1"].contains(&host.as_str()) {
        return true;
    }
    // A wildcard bind is reachable under any name; the token is then the guard.
    if opts.is_wildcard() {
        return true;
    }
    let bind = opts.bind.trim_matches(['[', ']']).to_ascii_lowercase();
    host == bind
}

/// Same-origin check: `Origin` must be `http(s)://<the Host header>`.
fn origin_matches(origin: &str, host: &str) -> bool {
    ["http://", "https://"]
        .iter()
        .filter_map(|p| origin.strip_prefix(p))
        .any(|auth| auth.eq_ignore_ascii_case(host))
}

fn request_host(req: &Request) -> Option<String> {
    req.headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| req.uri().authority().map(|a| a.to_string()))
}

fn is_safe_method(m: &Method) -> bool {
    matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn check_api_request(req: &Request, host: &str, opts: &WebOptions) -> ApiResult<()> {
    let h = req.headers();
    if h.get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == "cross-site")
    {
        return Err(ApiError::forbidden("cross-site requests are not allowed"));
    }
    match h.get(header::ORIGIN).map(HeaderValue::to_str) {
        Some(Ok(origin)) => {
            if !origin_matches(origin, host) {
                return Err(ApiError::forbidden("the Origin does not match this server"));
            }
        }
        Some(Err(_)) => return Err(ApiError::forbidden("invalid Origin header")),
        // Browsers omit Origin on same-origin reads. Without one, only non-browser clients
        // on a loopback server may change state or open the stream.
        None => {
            // `EventSource` on the same origin sends no Origin, but browsers do send
            // `Sec-Fetch-Site: same-origin`; a client with neither is not a browser tab.
            let stream = req.uri().path() == "/api/v1/stream";
            let same_origin_fetch = h
                .get("sec-fetch-site")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v == "same-origin");
            let needs_origin = !is_safe_method(req.method()) || (stream && !same_origin_fetch);
            if needs_origin && !opts.is_loopback() {
                return Err(ApiError::forbidden("an Origin header is required"));
            }
        }
    }
    if !is_safe_method(req.method()) {
        let has_body = h
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v != "0")
            || h.contains_key(header::TRANSFER_ENCODING);
        let json = h
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';')
                    .next()
                    .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
            });
        if has_body && !json {
            return Err(ApiError::unsupported_media_type());
        }
    }
    Ok(())
}

/// Outermost layer: host/origin/auth checks, then the response headers.
pub async fn guard(State(st): State<AppState>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let is_api = path == "/api" || path.starts_with("/api/");
    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map_or(IpAddr::from([127, 0, 0, 1]), |c| c.0.ip());
    req.extensions_mut().insert(ClientIp(ip));

    let denied = (|| -> ApiResult<()> {
        let host = request_host(&req).ok_or_else(|| ApiError::forbidden("missing Host header"))?;
        if !host_allowed(&host, &st.opts) {
            return Err(ApiError::forbidden("the Host header is not allowed"));
        }
        if is_api {
            check_api_request(&req, &host, &st.opts)?;
            if !TOKEN_PATHS.contains(&path.as_str()) {
                let token = st
                    .opts
                    .token
                    .as_deref()
                    .filter(|t| !t.is_empty())
                    .ok_or_else(ApiError::unauthorized)?;
                // Valid credentials always pass; the limiter only gates guessing.
                if !authorized(req.headers(), token) {
                    let h = req.headers();
                    let has_cookie = cookie(h, COOKIE_NAME).is_some();
                    if h.contains_key(header::AUTHORIZATION) || has_cookie {
                        st.limiter.check(ip).map_err(ApiError::rate_limited)?;
                        st.limiter.record_failure(ip);
                    }
                    if has_cookie {
                        return Err(ApiError::unauthorized_clearing_cookie());
                    }
                    return Err(ApiError::unauthorized());
                }
            }
        }
        Ok(())
    })();
    let mut res = match denied {
        Ok(()) => next.run(req).await,
        Err(e) => e.into_response(),
    };
    let h = res.headers_mut();
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if is_api {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    } else if !h.contains_key(header::CONTENT_SECURITY_POLICY) {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        );
    }
    res
}

/// `POST /api/v1/token` (and its alias `/auth`): trades the token for the session cookie.
pub async fn post_token(
    State(st): State<AppState>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<TokenRequest>,
) -> ApiResult<Response> {
    let token = st
        .opts
        .token
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or_else(ApiError::unauthorized)?;
    st.limiter.check(ip).map_err(ApiError::rate_limited)?;
    if !ct_eq(&body.token, token) {
        st.limiter.record_failure(ip);
        return Err(ApiError::unauthorized());
    }
    let secure = st.opts.trust_forwarded_proto
        && headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("https"));
    let cookie = format!(
        "{COOKIE_NAME}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={COOKIE_MAX_AGE}{}",
        cookie_value(token),
        if secure { "; Secure" } else { "" }
    );
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(ApiError::internal)?,
    );
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(bind: &str, port: u16) -> WebOptions {
        WebOptions {
            bind: bind.into(),
            port,
            ..WebOptions::default()
        }
    }

    #[test]
    fn host_allowlist() {
        let o = opts("127.0.0.1", 7878);
        for ok in [
            "localhost:7878",
            "127.0.0.1:7878",
            "[::1]:7878",
            "LOCALHOST",
            "localhost",
        ] {
            assert!(host_allowed(ok, &o), "{ok}");
        }
        for bad in [
            "evil.example:7878",
            "localhost:80",
            "localhost.evil.example",
            "127.0.0.1:7878@evil.example",
            "",
            "::1:7878",
        ] {
            assert!(!host_allowed(bad, &o), "{bad}");
        }
        let lan = opts("192.168.1.5", 7878);
        assert!(host_allowed("192.168.1.5:7878", &lan));
        assert!(!host_allowed("192.168.1.6:7878", &lan));
        assert!(host_allowed("anything.lan:7878", &opts("0.0.0.0", 7878)));
        assert!(!host_allowed("anything.lan:1", &opts("0.0.0.0", 7878)));
    }

    #[test]
    fn origin_must_equal_host() {
        assert!(origin_matches("http://localhost:7878", "localhost:7878"));
        assert!(origin_matches("https://LOCALHOST:7878", "localhost:7878"));
        assert!(!origin_matches("http://evil.example", "localhost:7878"));
        assert!(!origin_matches("null", "localhost:7878"));
        assert!(!origin_matches("ftp://localhost:7878", "localhost:7878"));
    }

    #[test]
    fn rate_limiter_blocks_after_five_failures_per_ip() {
        let l = RateLimiter::default();
        let a = IpAddr::from([10, 0, 0, 1]);
        let b = IpAddr::from([10, 0, 0, 2]);
        for _ in 0..MAX_FAILURES {
            assert!(l.check(a).is_ok());
            l.record_failure(a);
        }
        assert!(l.check(a).is_err());
        assert!(l.check(b).is_ok());
    }

    #[test]
    fn rate_limiter_evicts_only_the_oldest_ip_at_capacity() {
        let limiter = RateLimiter::default();
        let now = Instant::now();
        let oldest = IpAddr::from([10, 0, 0, 0]);
        {
            let mut map = limiter.failures.lock().unwrap();
            for n in 0..MAX_TRACKED_IPS {
                let ip = IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + n as u32));
                let at = now - Duration::from_secs(if n == 0 { 2 } else { 1 });
                map.insert(ip, VecDeque::from(vec![at; MAX_FAILURES]));
            }
        }
        let new = IpAddr::from([192, 0, 2, 1]);
        limiter.record_failure(new);
        assert!(limiter.check(oldest).is_ok());
        for n in 1..MAX_TRACKED_IPS {
            let ip = IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + n as u32));
            assert!(limiter.check(ip).is_err(), "active failures lost for {ip}");
        }
        assert_eq!(limiter.failures.lock().unwrap().len(), MAX_TRACKED_IPS);
    }

    #[test]
    fn cookie_value_is_not_the_token() {
        let v = cookie_value("secret");
        assert!(!v.contains("secret"));
        assert_eq!(v.len(), 64);
        assert_ne!(v, cookie_value("secret2"));
    }
}

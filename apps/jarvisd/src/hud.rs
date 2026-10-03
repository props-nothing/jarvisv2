//! The heads-up display: one static page the daemon serves to a browser on this machine.
//!
//! `P3-036`. `jarvis watch` is a terminal view; a real assistant shows its state and its work where a person can
//! glance at it. This is that surface: an orb whose colour says idle, working or **waiting for you**, and the same
//! four lists as `jarvis watch` (waiting for you, working, scheduled, recent), with a **Stop** button per run and for
//! everything.
//!
//! # What is public, and why that is safe
//!
//! Exactly two paths are served without the bearer credential, and only for `GET`: the page and its script
//! ([`is_public_asset`]). They are static text that contains **no data and no secret**; every number on the screen is
//! fetched from the authenticated API by the script, with a credential the page was opened with. Everything else,
//! including a `POST` to these same paths, still requires the credential.
//!
//! # How the page gets the credential
//!
//! `jarvis hud` opens `http://127.0.0.1:PORT/hud#token=…`. A URL **fragment** is never sent to a server and never
//! appears in a request log; the script moves it to the tab's session storage and removes it from the address bar at
//! once. It is a local, same-user, loopback credential, the same one the CLI reads from the profile; the limits are
//! recorded in `ADR-0135`.
//!
//! # What the page cannot do
//!
//! **Approve.** A decision needs the one-time code the daemon delivers to a private file in the profile's state
//! directory (`ADR-0018`), which a browser cannot read, and this slice does not weaken that: the page shows the exact
//! command instead. Stopping work needs no such code and is offered.
//!
//! # Content security
//!
//! Every response carries a policy that permits only this origin's script, no framing, no base URI and no form
//! posts, and the script inserts daemon-supplied text with `textContent` only (a run's objective and answer are model
//! output). A test refuses markup insertion in the script.

use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

const PAGE: &str = include_str!("hud/index.html");
const SCRIPT: &str = include_str!("hud/hud.js");
const STYLE: &str = include_str!("hud/hud.css");

/// The path of the page.
pub const PAGE_PATH: &str = "/hud";

/// The path of the page's script.
pub const SCRIPT_PATH: &str = "/hud.js";

/// The path of the page's stylesheet.
pub const STYLE_PATH: &str = "/hud.css";

/// The policy sent with every response of this module.
const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
connect-src 'self'; media-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The browser features the page may use: the microphone (voice input) and nothing else.
const PERMISSIONS_POLICY: &str = "microphone=(self), camera=(), geolocation=(), payment=(), usb=()";

/// Whether a request is for one of the three static assets, which are served without the bearer credential.
///
/// Exact paths and `GET` only: a prefix match or another method would widen what is public without anyone deciding
/// to.
#[must_use]
pub fn is_public_asset(method: &Method, path: &str) -> bool {
    method == Method::GET && (path == PAGE_PATH || path == SCRIPT_PATH || path == STYLE_PATH)
}

fn asset(body: &'static str, content_type: &'static str) -> Response {
    let mut response = (StatusCode::OK, body).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static(PERMISSIONS_POLICY),
    );
    response
}

/// `GET /hud`
pub async fn page() -> Response {
    asset(PAGE, "text/html; charset=utf-8")
}

/// `GET /hud.js`
pub async fn script() -> Response {
    asset(SCRIPT, "text/javascript; charset=utf-8")
}

/// `GET /hud.css`
pub async fn style() -> Response {
    asset(STYLE, "text/css; charset=utf-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_exact_paths_are_public_and_only_for_get() {
        assert!(is_public_asset(&Method::GET, "/hud"));
        assert!(is_public_asset(&Method::GET, "/hud.js"));
        assert!(is_public_asset(&Method::GET, "/hud.css"));
        for path in [
            "/hud/",
            "/hudx",
            "/hud.js/",
            "/api/v1/runs",
            "/",
            "/health/live",
            "/HUD",
        ] {
            assert!(!is_public_asset(&Method::GET, path), "{path}");
        }
        assert!(!is_public_asset(&Method::POST, "/hud"));
        assert!(!is_public_asset(&Method::DELETE, "/hud.js"));
    }

    #[test]
    fn the_script_never_inserts_daemon_text_as_markup() {
        for forbidden in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "eval(",
            "new Function",
        ] {
            assert!(
                !SCRIPT.contains(forbidden),
                "the script must not use {forbidden}"
            );
        }
        assert!(
            !PAGE.contains("<script>"),
            "the page must not carry inline script, which the policy forbids"
        );
    }

    #[test]
    fn voice_stays_in_the_browser_and_the_page_loads_nothing_from_elsewhere() {
        for external in [
            "http://",
            "https://",
            "WebSocket",
            "XMLHttpRequest",
            "sendBeacon",
        ] {
            assert!(
                !SCRIPT.contains(external) && !PAGE.contains(external),
                "the page must load and send nothing outside this origin: {external}"
            );
        }
        // Speech is the browser's own recognizer and synthesizer; no audio is captured and posted anywhere.
        assert!(
            !SCRIPT.contains("MediaRecorder"),
            "audio must never be recorded by the page"
        );
        assert!(
            !PAGE.contains(" style="),
            "the policy forbids inline style attributes"
        );
    }

    #[test]
    fn the_credential_is_read_from_the_fragment_and_removed_from_the_address_bar() {
        assert!(SCRIPT.contains("location.hash"));
        assert!(SCRIPT.contains("history.replaceState"));
        assert!(
            !SCRIPT.contains("location.search"),
            "a query string is logged by servers and proxies"
        );
    }
}

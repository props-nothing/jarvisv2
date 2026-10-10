//! The console: the static page the daemon serves to a browser on this machine.
//!
//! `P3-036`. A real assistant shows its state and its work where a person can glance at it, and lets them answer it.
//! This is that surface: a face whose colour says idle, working, listening, speaking or **waiting for you**, a
//! streaming conversation with voice, the lists from `jarvis watch` (waiting for you, working, scheduled, recent),
//! **Approve** and **Deny** for anything waiting, and **Stop** per run and for everything (`ADR-0135`, `ADR-0136`).
//!
//! # What is public, and why that is safe
//!
//! Exactly five paths are served without the bearer credential, and only for `GET`: the page (`/` and its alias
//! `/hud`), its script, the head's script and its stylesheet ([`is_public_asset`]). They are static text that contains **no data and no
//! secret**; every number on the screen is fetched from the authenticated API by the script, with a credential the
//! page was opened with. Everything else, including a `POST` to these same paths, still requires the credential.
//!
//! # How the page gets the credential
//!
//! `jarvis hud` opens `http://127.0.0.1:PORT/#token=…`. A URL **fragment** is never sent to a server and never
//! appears in a request log; the script moves it to the tab's session storage and removes it from the address bar at
//! once. It is a local, same-user, loopback credential, the same one the CLI reads from the profile.
//!
//! # Content security
//!
//! Every response carries a policy that permits only this origin's script and style, audio only from a `blob:` the page
//! made from the daemon's own speech response, no framing, no base URI and no form posts, and the script inserts daemon-supplied text with `textContent` only (a run's objective and answer are
//! model output). Tests refuse markup insertion, inline script or style, and any outside origin.
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

const PAGE: &str = include_str!("hud/index.html");
const SCRIPT: &str = include_str!("hud/hud.js");
const HEAD: &str = include_str!("hud/head.js");
const MISSION: &str = include_str!("hud/mission.js");
const SETTINGS: &str = include_str!("hud/settings.js");
const STYLE: &str = include_str!("hud/hud.css");

/// The path of the page.
/// The console is the daemon's main page.
pub const PAGE_PATH: &str = "/";

/// The older address of the console, kept so a bookmark keeps working.
pub const ALIAS_PATH: &str = "/hud";

/// The path of the page's script.
pub const SCRIPT_PATH: &str = "/hud.js";

/// The path of the head: the face mesh and the code that draws it.
pub const HEAD_PATH: &str = "/head.js";

/// The path of the live mission view: the feed of what JARVIS is doing, its sources, and the animation around the face.
pub const MISSION_PATH: &str = "/mission.js";

/// The path of the settings dialog's script.
pub const SETTINGS_PATH: &str = "/settings.js";

/// The path of the page's stylesheet.
pub const STYLE_PATH: &str = "/hud.css";

/// The policy sent with every response of this module.
const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
connect-src 'self'; media-src blob:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The browser features the page may use: the microphone (voice input) and nothing else.
const PERMISSIONS_POLICY: &str = "microphone=(self), camera=(), geolocation=(), payment=(), usb=()";

/// Whether a request is for one of the static assets (the page at `/` and `/hud`, its script and its stylesheet), which
/// are served without the bearer credential.
///
/// Exact paths and `GET` only: a prefix match or another method would widen what is public without anyone deciding
/// to.
#[must_use]
pub fn is_public_asset(method: &Method, path: &str) -> bool {
    method == Method::GET
        && (path == PAGE_PATH
            || path == ALIAS_PATH
            || path == SCRIPT_PATH
            || path == HEAD_PATH
            || path == MISSION_PATH
            || path == SETTINGS_PATH
            || path == STYLE_PATH)
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

/// `GET /` and `GET /hud`
pub async fn page() -> Response {
    asset(PAGE, "text/html; charset=utf-8")
}

/// `GET /hud.js`
pub async fn script() -> Response {
    asset(SCRIPT, "text/javascript; charset=utf-8")
}

/// `GET /head.js`
pub async fn head() -> Response {
    asset(HEAD, "text/javascript; charset=utf-8")
}

/// `GET /mission.js`
pub async fn mission() -> Response {
    asset(MISSION, "text/javascript; charset=utf-8")
}

/// `GET /settings.js`
pub async fn settings() -> Response {
    asset(SETTINGS, "text/javascript; charset=utf-8")
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
        assert!(is_public_asset(&Method::GET, "/"));
        assert!(is_public_asset(&Method::GET, "/hud"));
        assert!(is_public_asset(&Method::GET, "/hud.js"));
        assert!(is_public_asset(&Method::GET, "/head.js"));
        assert!(is_public_asset(&Method::GET, "/mission.js"));
        assert!(is_public_asset(&Method::GET, "/settings.js"));
        assert!(is_public_asset(&Method::GET, "/hud.css"));
        for path in [
            "/hud/",
            "/hudx",
            "/hud.js/",
            "/api/v1/runs",
            "/index.html",
            "//",
            "/health/live",
            "/HUD",
        ] {
            assert!(!is_public_asset(&Method::GET, path), "{path}");
        }
        assert!(!is_public_asset(&Method::POST, "/hud"));
        assert!(!is_public_asset(&Method::POST, "/"));
        assert!(!is_public_asset(&Method::DELETE, "/hud.js"));
        assert!(!is_public_asset(&Method::POST, "/head.js"));
        assert!(!is_public_asset(&Method::POST, "/mission.js"));
        assert!(!is_public_asset(&Method::POST, "/settings.js"));
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
                !SCRIPT.contains(forbidden)
                    && !HEAD.contains(forbidden)
                    && !MISSION.contains(forbidden)
                    && !SETTINGS.contains(forbidden),
                "the scripts must not use {forbidden}"
            );
        }
        assert!(
            !PAGE.contains("<script>"),
            "the page must not carry inline script, which the policy forbids"
        );
    }

    /// Links the mission view offers come from a model or a page, so they are plain web addresses, open in a new tab, and carry no
    /// referrer or opener.
    #[test]
    fn the_mission_view_only_links_plain_web_addresses_safely() {
        assert!(MISSION.contains("rel = \"noopener noreferrer\""));
        assert!(MISSION.contains("\"http:\" && url.protocol !== \"https:\""));
        assert!(MISSION.contains("url.username || url.password"));
        assert!(PAGE.contains("/mission.js"));
        assert!(PAGE.contains("/settings.js"));
        // hud.js calls JarvisSettings while it loads, so the settings script must come first.
        let at = |name: &str| PAGE.find(name).unwrap_or(usize::MAX);
        assert!(at("/settings.js") < at("/hud.js"));
        assert!(SCRIPT.contains("JarvisSettings(") && SETTINGS.contains("window.JarvisSettings"));
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
                !SCRIPT.contains(external)
                    && !HEAD.contains(external)
                    && !MISSION.contains(external)
                    && !SETTINGS.contains(external)
                    && !PAGE.contains(external),
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

    /// The head's geometry is data inside a script; a truncated or hand-edited copy would draw garbage with no error, so
    /// its shape is pinned: 468 vertices, 898 triangles, every index in range, and the attribution kept.
    #[test]
    fn the_head_mesh_is_the_expected_shape_and_keeps_its_attribution() {
        let start = HEAD.find("var MESH = ").map(|at| at + "var MESH = ".len());
        let start = start.unwrap_or_else(|| panic!("the mesh is not in the head script"));
        let end = HEAD[start..]
            .find("};")
            .unwrap_or_else(|| panic!("the mesh is not terminated"));
        let mesh: serde_json::Value = serde_json::from_str(&HEAD[start..=start + end])
            .unwrap_or_else(|error| panic!("the mesh is not valid JSON: {error}"));
        let vertices = mesh["v"].as_array().map_or(0, Vec::len);
        let indices: Vec<u64> = mesh["f"]
            .as_array()
            .map(|items| items.iter().filter_map(serde_json::Value::as_u64).collect())
            .unwrap_or_default();
        assert_eq!(vertices, 468 * 3);
        assert_eq!(indices.len(), 898 * 3);
        assert!(indices.iter().all(|&index| index < 468));
        assert!(HEAD.contains("Apache License 2.0") && HEAD.contains("MediaPipe Authors"));
    }

    /// Audio may only come from a `blob:` the page makes itself, and nothing else in the policy is widened to allow it.
    #[test]
    fn the_policy_allows_blob_audio_and_no_outside_source() {
        assert!(CONTENT_SECURITY_POLICY.contains("media-src blob:;"));
        for loosened in [
            "*",
            "http:",
            "https:",
            "data:",
            "unsafe-inline",
            "unsafe-eval",
        ] {
            assert!(
                !CONTENT_SECURITY_POLICY.contains(loosened),
                "the policy must not contain {loosened}"
            );
        }
        assert!(CONTENT_SECURITY_POLICY.contains("default-src 'none'"));
        assert!(CONTENT_SECURITY_POLICY.contains("connect-src 'self'"));
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

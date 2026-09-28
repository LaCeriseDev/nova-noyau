//! Les réponses HTTP que tout service NOVA renvoie de la même façon (feature `axum`).
//!
//! Le 424 mérite un mot : Cloudflare remplace toute réponse 5xx de l'origine par sa propre
//! page HTML, et un message envoyé en 502 n'arriverait jamais à l'écran. « Failed
//! Dependency » dit d'ailleurs la vérité — c'est l'API en aval qui a lâché, pas nous.

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::acces::{Acces, Allowlist, Identite, Refus};
use crate::quota::{Jauge, PlafondAtteint};

pub fn connexion_requise() -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "Connexion requise"}))).into_response()
}

pub fn requete_invalide(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response()
}

pub fn plafond_atteint(p: PlafondAtteint) -> Response {
    (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error": p.to_string(), "quota": p.0}))).into_response()
}

pub fn cerveau_indisponible(e: &dyn std::fmt::Display) -> Response {
    tracing::error!("cerveau indisponible : {e}");
    (StatusCode::FAILED_DEPENDENCY, Json(json!({"error": format!("Le cerveau n'a pas répondu : {e}")}))).into_response()
}

pub fn interdit(message: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({"error": message}))).into_response()
}

/// `{"quota": {...}}` à joindre aux réponses.
pub fn avec_jauge(mut corps: serde_json::Value, jauge: Jauge) -> Response {
    corps["quota"] = json!(jauge);
    Json(corps).into_response()
}

/// Identifie depuis des en-têtes axum. Refus → 401, déjà tracé.
pub async fn identifier(acces: &Acces, allow: &Allowlist, headers: &HeaderMap) -> Result<Identite, Response> {
    let lire = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    acces.identifier(&lire, allow).await.map_err(|_: Refus| connexion_requise())
}

/// L'index d'une SPA ne doit jamais être mis en cache : sinon une ancienne app.js fait
/// passer une mise à jour pour un bug.
pub async fn no_cache(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-cache"));
    resp
}

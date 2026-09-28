//! Le cerveau : l'API Messages, sans outil, bornée par un schéma.
//!
//! Une seule mise en œuvre pour tous les appels d'un projet. Mia en avait quatre copies
//! presque identiques (capture, texte libre, JSON libre, discussion) : la même
//! authentification, le même timeout, le même comptage d'usage, la même lecture des
//! erreurs. Ici c'est un [`Appel`] qu'on compose, et un [`Cerveau`] qui l'envoie.

use serde_json::{json, Value};
use thiserror::Error;

/// Usage réel renvoyé par l'API. C'est ce qu'on compte, jamais une estimation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub tokens_caches: i64,
}

impl Usage {
    /// Ce qui compte pour un plafond journalier : entrée + sortie.
    pub fn total(&self) -> i64 {
        self.tokens_in + self.tokens_out
    }

    pub fn depuis(v: &Value) -> Usage {
        let lire = |k: &str| v.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
        Usage {
            tokens_in: lire("input_tokens"),
            tokens_out: lire("output_tokens"),
            tokens_caches: lire("cache_read_input_tokens"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Reponse {
    /// Le texte du premier bloc `text`.
    pub texte: String,
    /// Le JSON décodé quand l'appel imposait un schéma.
    pub json: Option<Value>,
    pub usage: Usage,
}

#[derive(Debug, Error)]
pub enum ErreurCerveau {
    #[error("API Anthropic ({statut}) : {detail}")]
    Api { statut: u16, detail: String },
    #[error("Le modèle a décliné cette demande.")]
    Refus,
    #[error("aucun bloc texte dans la réponse")]
    SansTexte,
    #[error("JSON de sortie illisible : {0}")]
    JsonIllisible(String),
    #[error("réseau : {0}")]
    Reseau(#[from] reqwest::Error),
}

/// Un appel à composer : système (mis en cache), fil de messages, schéma de sortie.
#[derive(Debug, Clone, Default)]
pub struct Appel {
    systeme: Option<String>,
    messages: Vec<(String, String)>,
    schema: Option<Value>,
    max_tokens: Option<u32>,
    effort: Option<String>,
}

impl Appel {
    pub fn nouveau() -> Self {
        Self::default()
    }

    /// Le prompt système, marqué `ephemeral` : payé une fois, relu en cache ensuite.
    pub fn systeme(mut self, texte: impl Into<String>) -> Self {
        self.systeme = Some(texte.into());
        self
    }

    /// Le fil (rôle, contenu), dans l'ordre. `user` / `assistant` uniquement.
    pub fn fil(mut self, fil: &[(String, String)]) -> Self {
        self.messages.extend(fil.iter().cloned());
        self
    }

    pub fn utilisateur(mut self, texte: impl Into<String>) -> Self {
        self.messages.push(("user".into(), texte.into()));
        self
    }

    /// Impose un schéma JSON à la sortie : la réponse est validée par l'API, puis
    /// décodée dans [`Reponse::json`].
    pub fn schema(mut self, schema: Value) -> Self {
        self.schema = Some(schema);
        self
    }

    pub fn max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = Some(n);
        self
    }

    /// Surcharge l'effort du cerveau pour cet appel seulement.
    pub fn effort(mut self, effort: impl Into<String>) -> Self {
        self.effort = Some(effort.into());
        self
    }

    /// Le corps envoyé, exposé pour les tests et les journaux.
    pub fn corps(&self, model: &str, effort_defaut: &str) -> Value {
        let mut body = json!({
            "model": model,
            "max_tokens": self.max_tokens.unwrap_or(4000),
            "messages": self.messages.iter().map(|(r, c)| json!({"role": r, "content": c})).collect::<Vec<_>>(),
        });
        if let Some(s) = &self.systeme {
            body["system"] = json!([{ "type": "text", "text": s, "cache_control": {"type": "ephemeral"} }]);
        }
        let mut cfg = json!({ "effort": self.effort.as_deref().unwrap_or(effort_defaut) });
        if let Some(sch) = &self.schema {
            cfg["format"] = json!({"type": "json_schema", "schema": sch});
        }
        body["output_config"] = cfg;
        body
    }
}

/// Le cerveau d'un projet : une clé dédiée (jamais l'abonnement de quelqu'un), un modèle,
/// un effort par défaut.
#[derive(Clone)]
pub struct Cerveau {
    api_key: String,
    pub model: String,
    pub effort: String,
    client: reqwest::Client,
    timeout: std::time::Duration,
}

impl Cerveau {
    pub fn nouveau(api_key: impl Into<String>, model: impl Into<String>, effort: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            effort: effort.into(),
            client: reqwest::Client::new(),
            timeout: std::time::Duration::from_secs(120),
        }
    }

    pub fn timeout(mut self, d: std::time::Duration) -> Self {
        self.timeout = d;
        self
    }

    /// Vrai si la clé est un jeton OAuth Claude Code : un dépannage, jamais un
    /// fonctionnement normal (dépense non attribuable, jeton qui expire).
    pub fn cle_est_oauth(&self) -> bool {
        self.api_key.starts_with("sk-ant-oat")
    }

    pub async fn envoyer(&self, appel: &Appel) -> Result<Reponse, ErreurCerveau> {
        let body = appel.corps(&self.model, &self.effort);
        let mut req = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json");
        req = if self.cle_est_oauth() {
            req.header("authorization", format!("Bearer {}", self.api_key))
                .header("anthropic-beta", "oauth-2025-04-20")
        } else {
            req.header("x-api-key", &self.api_key)
        };
        let resp = req.json(&body).timeout(self.timeout).send().await?;
        let statut = resp.status();
        let v: Value = resp.json().await?;
        if !statut.is_success() {
            let detail = v
                .pointer("/error/message")
                .and_then(|m| m.as_str())
                .unwrap_or("réponse inattendue de l'API")
                .to_string();
            return Err(ErreurCerveau::Api { statut: statut.as_u16(), detail });
        }
        Self::lire(&v, appel.schema.is_some())
    }

    /// Lit une réponse de l'API. Séparé de l'envoi pour être testable sans réseau.
    pub fn lire(v: &Value, attend_json: bool) -> Result<Reponse, ErreurCerveau> {
        if v.get("stop_reason").and_then(|s| s.as_str()) == Some("refusal") {
            return Err(ErreurCerveau::Refus);
        }
        let texte = v
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|blocs| blocs.iter().find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text")))
            .and_then(|b| b.get("text"))
            .and_then(|t| t.as_str())
            .ok_or(ErreurCerveau::SansTexte)?
            .to_string();
        let json = if attend_json {
            Some(serde_json::from_str(&texte).map_err(|e| ErreurCerveau::JsonIllisible(e.to_string()))?)
        } else {
            None
        };
        let usage = v.get("usage").map(Usage::depuis).unwrap_or_default();
        Ok(Reponse { texte, json, usage })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reponse_api(texte: &str) -> Value {
        json!({
            "content": [{"type": "text", "text": texte}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 120, "output_tokens": 30, "cache_read_input_tokens": 100}
        })
    }

    #[test]
    fn corps_avec_systeme_et_schema() {
        let a = Appel::nouveau().systeme("tu es sobre").utilisateur("bonjour").schema(json!({"type": "object"}));
        let c = a.corps("claude-x", "low");
        assert_eq!(c["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(c["output_config"]["format"]["type"], "json_schema");
        assert_eq!(c["output_config"]["effort"], "low");
        assert_eq!(c["messages"][0]["role"], "user");
    }

    #[test]
    fn corps_sans_schema_ne_declare_pas_de_format() {
        let c = Appel::nouveau().utilisateur("x").effort("high").corps("m", "low");
        assert!(c["output_config"].get("format").is_none());
        assert_eq!(c["output_config"]["effort"], "high");
        assert!(c.get("system").is_none());
    }

    #[test]
    fn lecture_texte_et_usage() {
        let r = Cerveau::lire(&reponse_api("salut"), false).unwrap();
        assert_eq!(r.texte, "salut");
        assert!(r.json.is_none());
        assert_eq!(r.usage.total(), 150);
        assert_eq!(r.usage.tokens_caches, 100);
    }

    #[test]
    fn lecture_json_valide_et_invalide() {
        let r = Cerveau::lire(&reponse_api(r#"{"a":1}"#), true).unwrap();
        assert_eq!(r.json.unwrap()["a"], 1);
        assert!(matches!(Cerveau::lire(&reponse_api("pas du json"), true), Err(ErreurCerveau::JsonIllisible(_))));
    }

    #[test]
    fn refus_et_sans_texte() {
        let mut v = reponse_api("x");
        v["stop_reason"] = json!("refusal");
        assert!(matches!(Cerveau::lire(&v, false), Err(ErreurCerveau::Refus)));
        let v = json!({"content": [], "stop_reason": "end_turn"});
        assert!(matches!(Cerveau::lire(&v, false), Err(ErreurCerveau::SansTexte)));
    }

    #[test]
    fn detecte_le_jeton_oauth() {
        assert!(Cerveau::nouveau("sk-ant-oat-abc", "m", "low").cle_est_oauth());
        assert!(!Cerveau::nouveau("sk-ant-api03-abc", "m", "low").cle_est_oauth());
    }
}

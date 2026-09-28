//! L'identité par Cloudflare Access.
//!
//! Elle n'est JAMAIS déclarative. Elle ne vient ni du prompt (« je suis Romain » n'est que
//! du texte), ni de l'en-tête `Cf-Access-Authenticated-User-Email` — posé par Cloudflare
//! mais forgeable par quiconque atteint l'origine sans passer par le proxy. La seule source
//! admise est la signature RS256 du jeton `Cf-Access-Jwt-Assertion`, vérifiée contre les
//! clés publiques de l'équipe.
//!
//! Une allowlist locale double la politique Cloudflare, volontairement : si celle-ci est un
//! jour élargie par erreur, l'origine refuse quand même les inconnus. Et chaque refus est
//! tracé en distinguant « jeton absent/invalide » de « email hors allowlist » — sans ça un
//! échec d'authentification est un mur noir.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

use crate::util::now_secs;

/// Délai minimum entre deux récupérations du jeu de clés, pour qu'un `kid` inconnu (ou
/// inventé) ne transforme pas chaque requête en appel réseau.
const JWKS_MIN_REFRESH_SECS: i64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identite {
    pub email: String,
    pub role: String,
}

impl Identite {
    pub fn est_proprietaire(&self) -> bool {
        self.role == "proprietaire"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refus {
    /// Pas de jeton, ou jeton mal signé / expiré / mauvais public.
    JetonInvalide { entete_present: bool },
    /// Jeton valide, mais l'email n'est pas admis ici.
    HorsAllowlist,
}

#[derive(Deserialize)]
struct Claims {
    email: Option<String>,
}
#[derive(Deserialize)]
struct Jwk {
    kid: String,
    n: String,
    e: String,
}
#[derive(Deserialize)]
struct JwkSet {
    keys: Vec<Jwk>,
}

/// Qui est admis : le propriétaire, et les invités.
#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    pub proprietaire: String,
    pub invites: HashSet<String>,
}

impl Allowlist {
    /// `proprietaire` et `invites` séparés par des virgules, normalisés en minuscules.
    pub fn depuis(proprietaire: &str, invites: &str) -> Self {
        Self {
            proprietaire: proprietaire.trim().to_lowercase(),
            invites: invites.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect(),
        }
    }

    /// Le rôle initial d'un email, ou None s'il n'est pas admis.
    pub fn role_de(&self, email: &str) -> Option<&'static str> {
        if !self.proprietaire.is_empty() && email == self.proprietaire {
            Some("proprietaire")
        } else if self.invites.contains(email) {
            Some("invite")
        } else {
            None
        }
    }
}

/// Le vérificateur d'un service : domaine d'équipe, audience de l'application, cache de clés.
pub struct Acces {
    team_domain: String,
    aud: String,
    /// En mode dev, l'email vient de l'en-tête `x-dev-email` (ou du propriétaire). JAMAIS en
    /// production : le service doit le crier au démarrage.
    pub dev_mode: bool,
    jwks: Mutex<HashMap<String, (String, String)>>,
    jwks_fetched_at: Mutex<i64>,
}

impl Acces {
    pub fn nouveau(team_domain: impl Into<String>, aud: impl Into<String>, dev_mode: bool) -> Self {
        Self {
            team_domain: team_domain.into().trim_end_matches('/').to_string(),
            aud: aud.into(),
            dev_mode,
            jwks: Mutex::new(HashMap::new()),
            jwks_fetched_at: Mutex::new(0),
        }
    }

    async fn cle_pour_kid(&self, kid: &str) -> Option<(String, String)> {
        if let Some(v) = self.jwks.lock().unwrap().get(kid).cloned() {
            return Some(v);
        }
        // Verrou relâché avant l'await : jamais un Mutex std à travers un point de suspension.
        {
            let mut last = self.jwks_fetched_at.lock().unwrap();
            if now_secs() - *last < JWKS_MIN_REFRESH_SECS {
                return None;
            }
            *last = now_secs();
        }
        let url = format!("{}/cdn-cgi/access/certs", self.team_domain);
        let set: JwkSet = reqwest::Client::new()
            .get(&url)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        let mut cache = self.jwks.lock().unwrap();
        for k in set.keys {
            cache.insert(k.kid, (k.n, k.e));
        }
        cache.get(kid).cloned()
    }

    /// Vérifie un jeton et renvoie l'email prouvé.
    pub async fn email_verifie(&self, jeton: &str) -> Option<String> {
        let header = decode_header(jeton).ok()?;
        if header.alg != Algorithm::RS256 {
            return None; // pas de négociation d'algorithme : RS256 ou rien.
        }
        let kid = header.kid?;
        let (n, e) = self.cle_pour_kid(&kid).await?;
        let key = DecodingKey::from_rsa_components(&n, &e).ok()?;
        let mut v = Validation::new(Algorithm::RS256);
        v.set_audience(&[self.aud.as_str()]);
        v.set_issuer(&[self.team_domain.as_str()]);
        v.validate_exp = true;
        let data = decode::<Claims>(jeton, &key, &v).ok()?;
        data.claims.email.map(|e| e.trim().to_lowercase())
    }

    /// Identifie l'appelant à partir des en-têtes bruts (`cf-access-jwt-assertion`, et
    /// `x-dev-email` en mode dev). Le rôle renvoyé est le rôle INITIAL de l'allowlist ; si
    /// le projet garde les rôles en base, c'est à lui de le relire (voir `touch_user` chez Mia).
    pub async fn identifier(&self, entetes: &(dyn Fn(&str) -> Option<String> + Sync), allow: &Allowlist) -> Result<Identite, Refus> {
        let email = if self.dev_mode {
            entetes("x-dev-email").unwrap_or_else(|| allow.proprietaire.clone()).trim().to_lowercase()
        } else {
            let jeton = entetes("cf-access-jwt-assertion");
            match jeton {
                Some(j) => match self.email_verifie(&j).await {
                    Some(e) => e,
                    None => {
                        tracing::warn!("jeton Cloudflare invalide (en-tête présent)");
                        return Err(Refus::JetonInvalide { entete_present: true });
                    }
                },
                None => {
                    tracing::warn!("jeton Cloudflare absent");
                    return Err(Refus::JetonInvalide { entete_present: false });
                }
            }
        };
        match allow.role_de(&email) {
            Some(role) => Ok(Identite { email, role: role.into() }),
            None => {
                tracing::warn!("« {email} » a un jeton valide mais n'est pas dans l'allowlist locale");
                Err(Refus::HorsAllowlist)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_et_roles() {
        let a = Allowlist::depuis(" Romain@X.fr ", "julia@x.fr, , Amie@x.fr");
        assert_eq!(a.role_de("romain@x.fr"), Some("proprietaire"));
        assert_eq!(a.role_de("julia@x.fr"), Some("invite"));
        assert_eq!(a.role_de("amie@x.fr"), Some("invite"));
        assert_eq!(a.role_de("inconnu@x.fr"), None);
        assert_eq!(Allowlist::depuis("", "").role_de(""), None);
    }

    #[tokio::test]
    async fn mode_dev_lit_l_entete_ou_le_proprietaire() {
        let acces = Acces::nouveau("https://eq.cloudflareaccess.com", "aud", true);
        let allow = Allowlist::depuis("romain@x.fr", "julia@x.fr");
        let sans = |_: &str| None::<String>;
        assert_eq!(acces.identifier(&sans, &allow).await.unwrap().role, "proprietaire");
        let avec = |k: &str| if k == "x-dev-email" { Some("Julia@x.fr".into()) } else { None };
        assert_eq!(acces.identifier(&avec, &allow).await.unwrap().email, "julia@x.fr");
        let hors = |k: &str| if k == "x-dev-email" { Some("z@x.fr".into()) } else { None };
        assert_eq!(acces.identifier(&hors, &allow).await, Err(Refus::HorsAllowlist));
    }

    #[tokio::test]
    async fn production_refuse_sans_jeton_et_jeton_bidon() {
        let acces = Acces::nouveau("https://eq.cloudflareaccess.com", "aud", false);
        let allow = Allowlist::depuis("romain@x.fr", "");
        let sans = |_: &str| None::<String>;
        assert_eq!(acces.identifier(&sans, &allow).await, Err(Refus::JetonInvalide { entete_present: false }));
        // alg:none — refusé avant tout appel réseau.
        let none = |k: &str| if k == "cf-access-jwt-assertion" { Some("eyJhbGciOiJub25lIn0.eyJlbWFpbCI6InJvbWFpbkB4LmZyIn0.".into()) } else { None };
        assert_eq!(acces.identifier(&none, &allow).await, Err(Refus::JetonInvalide { entete_present: true }));
        // Un en-tête déclaratif seul ne vaut rien.
        let declaratif = |k: &str| if k == "cf-access-authenticated-user-email" { Some("romain@x.fr".into()) } else { None };
        assert_eq!(acces.identifier(&declaratif, &allow).await, Err(Refus::JetonInvalide { entete_present: false }));
    }
}

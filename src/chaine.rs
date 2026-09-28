//! La chaîne d'écriture vérifiée : proposer → prévisualiser → confirmer → exécuter.
//!
//! Le modèle propose une action (un JSON conforme au contrat). Le serveur la met en forme,
//! la montre, et la **signe** : un jeton HMAC qui lie l'action exacte, la personne, une
//! échéance et un nonce. Confirmer, c'est renvoyer ce jeton : rien d'autre ne peut être
//! exécuté que ce qui a été montré, par la personne à qui ça a été montré, une seule fois,
//! dans le délai. Puis on journalise — l'audit fait partie de la chaîne, pas d'une option.
//!
//! Le noyau ne sait pas exécuter : l'exécution est au domaine. Il garantit seulement que
//! ce qui arrive à l'exécution est ce qui a été confirmé.

use hmac::{Hmac, Mac};
use rand::RngCore;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use thiserror::Error;

use crate::util::now_secs;

type HmacSha256 = Hmac<Sha256>;

/// Une action proposée et signée, prête à être montrée puis confirmée.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Jeton {
    /// L'action telle qu'elle sera exécutée — c'est elle qui est signée.
    pub action: Value,
    pub acteur: String,
    pub expire_le: i64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ErreurChaine {
    #[error("signature invalide : l'action ne correspond pas à ce qui a été montré")]
    Signature,
    #[error("jeton expiré")]
    Expire,
    #[error("jeton déjà utilisé")]
    DejaUtilise,
    #[error("ce jeton a été émis pour quelqu'un d'autre")]
    MauvaisActeur,
    #[error("base : {0}")]
    Base(String),
}

/// La chaîne d'un projet : un secret (32 octets au moins), une durée de vie des jetons.
#[derive(Clone)]
pub struct ChaineEcriture {
    secret: Vec<u8>,
    pub ttl_secs: i64,
}

impl ChaineEcriture {
    pub fn nouvelle(secret: impl AsRef<[u8]>, ttl_secs: i64) -> Self {
        let s = secret.as_ref();
        assert!(s.len() >= 32, "le secret de la chaîne d'écriture doit faire au moins 32 octets");
        Self { secret: s.to_vec(), ttl_secs }
    }

    /// Un secret tiré au sort, pour un service qui n'a pas besoin de survivre à un
    /// redémarrage (les jetons en cours tombent, c'est acceptable).
    pub fn secret_aleatoire() -> Vec<u8> {
        let mut s = vec![0u8; 32];
        rand::thread_rng().fill_bytes(&mut s);
        s
    }

    /// Les tables du noyau : jetons consommés (usage unique) et journal d'audit.
    pub fn init_db(conn: &Connection) -> Result<(), ErreurChaine> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS chaine_jetons_utilises (
                nonce      TEXT PRIMARY KEY,
                acteur     TEXT NOT NULL,
                utilise_le INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chaine_audit (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                acteur    TEXT NOT NULL,
                operation TEXT NOT NULL,
                action    TEXT NOT NULL,
                resultat  TEXT NOT NULL,
                cree_le   INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_chaine_audit ON chaine_audit(acteur, cree_le DESC);
            "#,
        )
        .map_err(|e| ErreurChaine::Base(e.to_string()))
    }

    fn charge(action: &Value, acteur: &str, expire_le: i64, nonce: &str) -> Vec<u8> {
        // Sérialisation canonique : serde_json trie les clés des `Map` par défaut
        // (feature `preserve_order` absente), donc deux actions égales signent pareil.
        format!("{}\n{}\n{}\n{}", serde_json::to_string(action).unwrap_or_default(), acteur, expire_le, nonce).into_bytes()
    }

    fn signer(&self, charge: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.secret).expect("HMAC accepte toute taille de clé");
        mac.update(charge);
        hex::encode(mac.finalize().into_bytes())
    }

    /// Proposer : signe l'action pour `acteur`. À montrer telle quelle (aperçu), puis à
    /// renvoyer pour confirmer.
    pub fn proposer(&self, action: Value, acteur: &str) -> Jeton {
        let mut n = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut n);
        let nonce = hex::encode(n);
        let expire_le = now_secs() + self.ttl_secs;
        let signature = self.signer(&Self::charge(&action, acteur, expire_le, &nonce));
        Jeton { action, acteur: acteur.to_string(), expire_le, nonce, signature }
    }

    /// Confirmer : vérifie signature, échéance, acteur et usage unique, puis marque le
    /// jeton consommé. Renvoie l'action à exécuter — celle qui a été signée, pas une autre.
    pub fn confirmer(&self, conn: &Connection, jeton: &Jeton, acteur: &str) -> Result<Value, ErreurChaine> {
        if jeton.acteur != acteur {
            return Err(ErreurChaine::MauvaisActeur);
        }
        let attendu = self.signer(&Self::charge(&jeton.action, &jeton.acteur, jeton.expire_le, &jeton.nonce));
        // Comparaison à temps constant : on ne laisse pas deviner la signature octet par octet.
        let mut mac = HmacSha256::new_from_slice(&self.secret).expect("HMAC");
        mac.update(&Self::charge(&jeton.action, &jeton.acteur, jeton.expire_le, &jeton.nonce));
        let recu = hex::decode(&jeton.signature).map_err(|_| ErreurChaine::Signature)?;
        if mac.verify_slice(&recu).is_err() || attendu.len() != jeton.signature.len() {
            return Err(ErreurChaine::Signature);
        }
        if now_secs() > jeton.expire_le {
            return Err(ErreurChaine::Expire);
        }
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO chaine_jetons_utilises(nonce, acteur, utilise_le) VALUES(?1, ?2, ?3)",
                params![jeton.nonce, acteur, now_secs()],
            )
            .map_err(|e| ErreurChaine::Base(e.to_string()))?;
        if inserted == 0 {
            return Err(ErreurChaine::DejaUtilise);
        }
        Ok(jeton.action.clone())
    }

    /// Journaliser : ce qui a été exécuté, par qui, avec quel résultat.
    pub fn journaliser(conn: &Connection, acteur: &str, operation: &str, action: &Value, resultat: &str) -> Result<i64, ErreurChaine> {
        conn.execute(
            "INSERT INTO chaine_audit(acteur, operation, action, resultat, cree_le) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![acteur, operation, action.to_string(), resultat, now_secs()],
        )
        .map_err(|e| ErreurChaine::Base(e.to_string()))?;
        Ok(conn.last_insert_rowid())
    }

    /// Purge des nonces plus vieux que la durée de vie : ils ne peuvent plus servir.
    pub fn purger(&self, conn: &Connection) -> Result<usize, ErreurChaine> {
        conn.execute(
            "DELETE FROM chaine_jetons_utilises WHERE utilise_le < ?1",
            params![now_secs() - self.ttl_secs * 2],
        )
        .map_err(|e| ErreurChaine::Base(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chaine() -> (ChaineEcriture, Connection) {
        let c = ChaineEcriture::nouvelle(b"un-secret-de-test-assez-long-pour-hmac-32", 60);
        let conn = Connection::open_in_memory().unwrap();
        ChaineEcriture::init_db(&conn).unwrap();
        (c, conn)
    }

    #[test]
    fn aller_retour_nominal() {
        let (c, conn) = chaine();
        let j = c.proposer(json!({"op": "create", "entite": "technicien", "nom": "Ana"}), "chef@x.fr");
        let a = c.confirmer(&conn, &j, "chef@x.fr").unwrap();
        assert_eq!(a["nom"], "Ana");
        ChaineEcriture::journaliser(&conn, "chef@x.fr", "create", &a, "ok").unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM chaine_audit", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn action_modifiee_apres_apercu() {
        let (c, conn) = chaine();
        let mut j = c.proposer(json!({"op": "create", "nom": "Ana"}), "chef@x.fr");
        j.action["nom"] = json!("Bob");
        assert_eq!(c.confirmer(&conn, &j, "chef@x.fr"), Err(ErreurChaine::Signature));
    }

    #[test]
    fn signature_forgee() {
        let (c, conn) = chaine();
        let mut j = c.proposer(json!({"op": "x"}), "a@x.fr");
        j.signature = "00".repeat(32);
        assert_eq!(c.confirmer(&conn, &j, "a@x.fr"), Err(ErreurChaine::Signature));
        j.signature = "zz".into();
        assert_eq!(c.confirmer(&conn, &j, "a@x.fr"), Err(ErreurChaine::Signature));
    }

    #[test]
    fn usage_unique_et_mauvais_acteur() {
        let (c, conn) = chaine();
        let j = c.proposer(json!({"op": "x"}), "a@x.fr");
        assert_eq!(c.confirmer(&conn, &j, "b@x.fr"), Err(ErreurChaine::MauvaisActeur));
        assert!(c.confirmer(&conn, &j, "a@x.fr").is_ok());
        assert_eq!(c.confirmer(&conn, &j, "a@x.fr"), Err(ErreurChaine::DejaUtilise));
    }

    #[test]
    fn expiration() {
        let c = ChaineEcriture::nouvelle(b"un-secret-de-test-assez-long-pour-hmac-32", -1);
        let conn = Connection::open_in_memory().unwrap();
        ChaineEcriture::init_db(&conn).unwrap();
        let j = c.proposer(json!({"op": "x"}), "a@x.fr");
        assert_eq!(c.confirmer(&conn, &j, "a@x.fr"), Err(ErreurChaine::Expire));
    }

    #[test]
    fn deux_secrets_ne_se_reconnaissent_pas() {
        let (c1, conn) = chaine();
        let c2 = ChaineEcriture::nouvelle(b"un-autre-secret-de-test-assez-long-32-o", 60);
        let j = c1.proposer(json!({"op": "x"}), "a@x.fr");
        assert_eq!(c2.confirmer(&conn, &j, "a@x.fr"), Err(ErreurChaine::Signature));
    }
}

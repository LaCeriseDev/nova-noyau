//! Le contrat d'intent.
//!
//! Le modèle ne produit jamais de SQL ni d'UI : il produit un JSON, et ce JSON est vérifié
//! ICI, côté serveur, avant d'être exécuté — même si l'API a déjà validé la sortie contre
//! le schéma. On ne fait pas confiance à ce qui a traversé le réseau.
//!
//! Le validateur couvre le sous-ensemble de JSON Schema que les contrats NOVA emploient :
//! `object` (`properties`, `required`, `additionalProperties`), `array` (`items`,
//! `minItems`, `maxItems`), `string` (`enum`, `maxLength`), `number`, `integer`,
//! `boolean`, `null`, et `type` à choix multiples. Ce qu'il ne connaît pas, il l'ignore :
//! un contrat plus riche que lui ne fait pas échouer la validation, il la laisse passer
//! — c'est écrit, ne pas croire qu'un mot-clé exotique est vérifié.
//!
//! Convention d'honnêteté : un intent peut porter `{"cannot": "raison"}`. C'est la sortie
//! que le contrat doit prévoir pour que le modèle refuse proprement au lieu de replier vers
//! un résultat plausible et faux. [`est_refus`] la reconnaît.

use serde_json::Value;

/// Un écart entre la valeur et le contrat, avec son chemin (`/a/0/b`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ecart {
    pub chemin: String,
    pub raison: String,
}

impl std::fmt::Display for Ecart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} : {}", if self.chemin.is_empty() { "/" } else { &self.chemin }, self.raison)
    }
}

/// Valide `valeur` contre `schema`. Vide = conforme.
pub fn valider(schema: &Value, valeur: &Value) -> Vec<Ecart> {
    let mut ecarts = Vec::new();
    verifier(schema, valeur, "", &mut ecarts);
    ecarts
}

/// `{"cannot": "…"}` : le modèle dit qu'il ne peut pas. Renvoie la raison.
pub fn est_refus(intent: &Value) -> Option<&str> {
    intent.get("cannot").and_then(|c| c.as_str()).filter(|s| !s.trim().is_empty())
}

fn type_de(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_admis(attendu: &str, reel: &str) -> bool {
    attendu == reel || (attendu == "number" && reel == "integer")
}

fn verifier(schema: &Value, v: &Value, chemin: &str, out: &mut Vec<Ecart>) {
    let pousser = |out: &mut Vec<Ecart>, raison: String| out.push(Ecart { chemin: chemin.to_string(), raison });

    if let Some(t) = schema.get("type") {
        let reel = type_de(v);
        let ok = match t {
            Value::String(s) => type_admis(s, reel),
            Value::Array(ts) => ts.iter().any(|s| s.as_str().map(|s| type_admis(s, reel)).unwrap_or(false)),
            _ => true,
        };
        if !ok {
            pousser(out, format!("type {reel} au lieu de {t}"));
            return;
        }
    }

    if let Some(e) = schema.get("enum").and_then(|e| e.as_array()) {
        if !e.iter().any(|x| x == v) {
            pousser(out, format!("valeur hors de l'énumération : {v}"));
        }
    }

    match v {
        Value::String(s) => {
            if let Some(max) = schema.get("maxLength").and_then(|m| m.as_u64()) {
                if s.chars().count() as u64 > max {
                    pousser(out, format!("plus de {max} caractères"));
                }
            }
        }
        Value::Array(items) => {
            if let Some(min) = schema.get("minItems").and_then(|m| m.as_u64()) {
                if (items.len() as u64) < min {
                    pousser(out, format!("moins de {min} éléments"));
                }
            }
            if let Some(max) = schema.get("maxItems").and_then(|m| m.as_u64()) {
                if items.len() as u64 > max {
                    pousser(out, format!("plus de {max} éléments"));
                }
            }
            if let Some(sch) = schema.get("items") {
                for (i, it) in items.iter().enumerate() {
                    verifier(sch, it, &format!("{chemin}/{i}"), out);
                }
            }
        }
        Value::Object(map) => {
            let props = schema.get("properties").and_then(|p| p.as_object());
            if let Some(req) = schema.get("required").and_then(|r| r.as_array()) {
                for r in req.iter().filter_map(|r| r.as_str()) {
                    if !map.contains_key(r) {
                        pousser(out, format!("champ requis absent : {r}"));
                    }
                }
            }
            let ferme = schema.get("additionalProperties") == Some(&Value::Bool(false));
            for (k, val) in map {
                match props.and_then(|p| p.get(k)) {
                    Some(sch) => verifier(sch, val, &format!("{chemin}/{k}"), out),
                    None if ferme => out.push(Ecart { chemin: format!("{chemin}/{k}"), raison: "champ non prévu par le contrat".into() }),
                    None => {}
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn contrat() -> Value {
        json!({
            "type": "object",
            "properties": {
                "kind": {"type": "string", "enum": ["table", "kpi", "cannot"]},
                "filters": {"type": "array", "maxItems": 3, "items": {
                    "type": "object",
                    "properties": {"dim": {"type": "string"}, "val": {"type": ["string", "number"]}},
                    "required": ["dim", "val"], "additionalProperties": false
                }},
                "limit": {"type": "integer"},
                "cannot": {"type": "string"}
            },
            "required": ["kind"],
            "additionalProperties": false
        })
    }

    #[test]
    fn conforme() {
        let v = json!({"kind": "table", "filters": [{"dim": "ville", "val": "Lyon"}, {"dim": "n", "val": 3}], "limit": 10});
        assert!(valider(&contrat(), &v).is_empty());
    }

    #[test]
    fn champ_requis_et_champ_non_prevu() {
        let e = valider(&contrat(), &json!({"sql": "DROP TABLE x"}));
        let raisons: Vec<String> = e.iter().map(|x| x.to_string()).collect();
        assert!(raisons.iter().any(|r| r.contains("requis absent : kind")), "{raisons:?}");
        assert!(raisons.iter().any(|r| r.starts_with("/sql : champ non prévu")), "{raisons:?}");
    }

    #[test]
    fn enum_type_et_bornes() {
        let e = valider(&contrat(), &json!({"kind": "graphe", "limit": 2.5, "filters": [1, 2, 3, 4]}));
        let chemins: Vec<&str> = e.iter().map(|x| x.chemin.as_str()).collect();
        assert!(chemins.contains(&"/kind"));
        assert!(chemins.contains(&"/limit"));
        assert!(chemins.contains(&"/filters"));
        assert!(chemins.contains(&"/filters/0"));
    }

    #[test]
    fn integer_accepte_comme_number() {
        assert!(valider(&json!({"type": "number"}), &json!(3)).is_empty());
        assert!(!valider(&json!({"type": "integer"}), &json!(3.5)).is_empty());
    }

    #[test]
    fn refus_honnete() {
        assert_eq!(est_refus(&json!({"kind": "cannot", "cannot": "entité inconnue"})), Some("entité inconnue"));
        assert_eq!(est_refus(&json!({"kind": "table"})), None);
        assert_eq!(est_refus(&json!({"cannot": "  "})), None);
    }
}

//! Le « champ qui mange tout ».
//!
//! On ne sait pas d'avance ce que les gens vont vouloir noter. Plutôt que de figer un
//! schéma qu'on migrera dix fois, chaque entité porte un noyau de colonnes stables et une
//! table d'attributs libres (clé, valeur, type), le type étant inféré à l'écriture. En cas
//! de doute on garde `texte` : jamais de perte d'information.

use rusqlite::{params, Connection};
use serde_json::{Map, Value};

/// Types admis. `texte` est le repli.
pub const TYPES: [&str; 4] = ["texte", "nombre", "date", "booleen"];

/// Infère le type d'une valeur libre.
pub fn infer_type(v: &str) -> &'static str {
    let t = v.trim();
    // « 1 500 » et « 1,5 » sont des nombres écrits à la française.
    if !t.is_empty() && t.replace(' ', "").replace(',', ".").parse::<f64>().is_ok() {
        return "nombre";
    }
    let est_date = t.len() == 10
        && t.as_bytes().iter().enumerate().all(|(i, b)| if i == 4 || i == 7 { *b == b'-' } else { b.is_ascii_digit() });
    if est_date {
        return "date";
    }
    if matches!(t.to_lowercase().as_str(), "oui" | "non" | "true" | "false" | "vrai" | "faux") {
        return "booleen";
    }
    "texte"
}

/// Normalise une valeur pour son type : `oui/vrai/true` → `true`, nombres sans espace, dates
/// et textes tels quels. Une valeur qui ne colle pas à son type est renvoyée en `texte`.
pub fn normaliser(valeur: &str, ty: &str) -> (String, &'static str) {
    let t = valeur.trim();
    match ty {
        "nombre" => match t.replace(' ', "").replace(',', ".").parse::<f64>() {
            Ok(n) => (if n.fract() == 0.0 { format!("{}", n as i64) } else { n.to_string() }, "nombre"),
            Err(_) => (t.to_string(), "texte"),
        },
        "booleen" => match t.to_lowercase().as_str() {
            "oui" | "true" | "vrai" | "1" => ("true".into(), "booleen"),
            "non" | "false" | "faux" | "0" => ("false".into(), "booleen"),
            _ => (t.to_string(), "texte"),
        },
        "date" if infer_type(t) == "date" => (t.to_string(), "date"),
        _ => (t.to_string(), "texte"),
    }
}

/// Le SQL de la table d'attributs d'une entité : `<entite>_attributs(<entite>_id, cle, valeur, type)`.
pub fn schema_sql(entite: &str) -> String {
    format!(
        r#"CREATE TABLE IF NOT EXISTS {e}_attributs (
            {e}_id  INTEGER NOT NULL REFERENCES {e}s(id) ON DELETE CASCADE,
            cle     TEXT NOT NULL,
            valeur  TEXT NOT NULL,
            type    TEXT NOT NULL DEFAULT 'texte',
            PRIMARY KEY ({e}_id, cle)
        );"#,
        e = entite
    )
}

fn nom_sur(entite: &str) -> Result<(), rusqlite::Error> {
    if entite.is_empty() || !entite.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(rusqlite::Error::InvalidParameterName(format!("nom d'entité invalide : {entite}")));
    }
    Ok(())
}

/// Pose (ou remplace) les attributs d'une entité depuis un objet JSON. Les valeurs vides
/// sont ignorées, les autres sont typées et normalisées. Renvoie le nombre écrit.
pub fn poser(conn: &Connection, entite: &str, id: i64, attributs: &Value) -> rusqlite::Result<usize> {
    nom_sur(entite)?;
    let Some(map) = attributs.as_object() else { return Ok(0) };
    let mut n = 0;
    for (cle, val) in map {
        let brut = match val {
            Value::String(s) => s.clone(),
            Value::Null => continue,
            other => other.to_string(),
        };
        if brut.trim().is_empty() || cle.trim().is_empty() {
            continue;
        }
        let (valeur, ty) = normaliser(&brut, infer_type(&brut));
        conn.execute(
            &format!(
                "INSERT INTO {e}_attributs({e}_id, cle, valeur, type) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT({e}_id, cle) DO UPDATE SET valeur = ?3, type = ?4",
                e = entite
            ),
            params![id, cle.trim(), valeur, ty],
        )?;
        n += 1;
    }
    Ok(n)
}

/// Lit les attributs d'une entité en objet JSON typé : nombres en nombres, booléens en
/// booléens, le reste en chaînes.
pub fn lire(conn: &Connection, entite: &str, id: i64) -> rusqlite::Result<Map<String, Value>> {
    nom_sur(entite)?;
    let mut stmt = conn.prepare(&format!("SELECT cle, valeur, type FROM {e}_attributs WHERE {e}_id = ?1 ORDER BY cle", e = entite))?;
    let rows = stmt.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
    let mut out = Map::new();
    for row in rows {
        let (cle, valeur, ty) = row?;
        let v = match ty.as_str() {
            "nombre" => valeur.parse::<f64>().ok().and_then(|n| serde_json::Number::from_f64(n)).map(Value::Number).unwrap_or(Value::String(valeur)),
            "booleen" => Value::Bool(valeur == "true"),
            _ => Value::String(valeur),
        };
        out.insert(cle, v);
    }
    Ok(out)
}

/// Retire un attribut.
pub fn retirer(conn: &Connection, entite: &str, id: i64, cle: &str) -> rusqlite::Result<usize> {
    nom_sur(entite)?;
    conn.execute(&format!("DELETE FROM {e}_attributs WHERE {e}_id = ?1 AND cle = ?2", e = entite), params![id, cle])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE idees(id INTEGER PRIMARY KEY, texte TEXT);").unwrap();
        c.execute_batch(&schema_sql("idee")).unwrap();
        c.execute("INSERT INTO idees(id, texte) VALUES(1, 'x')", []).unwrap();
        c
    }

    #[test]
    fn inference() {
        assert_eq!(infer_type("12"), "nombre");
        assert_eq!(infer_type("3.5"), "nombre");
        assert_eq!(infer_type("2026-09-28"), "date");
        assert_eq!(infer_type("oui"), "booleen");
        assert_eq!(infer_type("Lyon"), "texte");
        assert_eq!(infer_type("2026-9-28"), "texte");
    }

    #[test]
    fn normalisation() {
        assert_eq!(normaliser("1 200,50", "nombre"), ("1200.5".into(), "nombre"));
        assert_eq!(normaliser("12", "nombre"), ("12".into(), "nombre"));
        assert_eq!(normaliser("Oui", "booleen"), ("true".into(), "booleen"));
        assert_eq!(normaliser("peut-être", "booleen"), ("peut-être".into(), "texte"));
    }

    #[test]
    fn poser_puis_lire() {
        let c = base();
        let n = poser(&c, "idee", 1, &json!({"budget": "1 500", "lieu": "Nantes", "vide": "", "urgent": "oui", "quand": "2026-10-01"})).unwrap();
        assert_eq!(n, 4);
        let a = lire(&c, "idee", 1).unwrap();
        assert_eq!(a["budget"], json!(1500.0));
        assert_eq!(a["urgent"], json!(true));
        assert_eq!(a["lieu"], json!("Nantes"));
        assert_eq!(a["quand"], json!("2026-10-01"));
        assert!(a.get("vide").is_none());
        poser(&c, "idee", 1, &json!({"budget": "1600"})).unwrap();
        assert_eq!(lire(&c, "idee", 1).unwrap()["budget"], json!(1600.0));
        retirer(&c, "idee", 1, "lieu").unwrap();
        assert!(lire(&c, "idee", 1).unwrap().get("lieu").is_none());
    }

    #[test]
    fn nom_d_entite_refuse_l_injection() {
        let c = base();
        assert!(poser(&c, "idee; DROP TABLE idees", 1, &json!({"a": "b"})).is_err());
    }
}

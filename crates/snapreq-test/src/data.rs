//! Data files for data-driven runs: one iteration per row.
//!
//! - CSV (RFC 4180): the first row names the variables; quoted fields may contain commas,
//!   quotes (`""`) and line breaks.
//! - JSON: an array of objects; strings are used as they are, other values as compact JSON.

use std::path::Path;

use serde_json::Value;
use snapreq_collection::Variables;

/// Reads `path` (`.csv` or `.json`) into one set of variables per row.
pub fn load(path: &Path) -> Result<Vec<Variables>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let rows = match ext.as_str() {
        "csv" => from_csv(&text),
        "json" => from_json(&text),
        _ => Err("data files must end in .csv or .json".into()),
    }
    .map_err(|e| format!("`{}`: {e}", path.display()))?;
    if rows.is_empty() {
        return Err(format!("`{}` has no rows", path.display()));
    }
    Ok(rows)
}

pub fn from_json(text: &str) -> Result<Vec<Variables>, String> {
    let doc: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let Value::Array(items) = doc else {
        return Err("expected an array of objects".into());
    };
    items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let Value::Object(map) = item else {
                return Err(format!("row {}: expected an object", i + 1));
            };
            let mut vars = Variables::default();
            for (k, v) in map {
                vars.set(
                    k,
                    match v {
                        Value::String(s) => s,
                        other => other.to_string(),
                    },
                );
            }
            Ok(vars)
        })
        .collect()
}

pub fn from_csv(text: &str) -> Result<Vec<Variables>, String> {
    let records = parse_csv(text.strip_prefix('\u{feff}').unwrap_or(text))?;
    let mut records = records.into_iter();
    let Some(header) = records.next() else {
        return Ok(Vec::new());
    };
    if header.iter().any(|h| h.trim().is_empty()) {
        return Err("every column needs a name in the first row".into());
    }
    records
        .enumerate()
        // A blank line (trailing newline, say) is not a row.
        .filter(|(_, r)| !(r.len() == 1 && r[0].is_empty()))
        .map(|(i, record)| {
            if record.len() != header.len() {
                return Err(format!(
                    "row {} has {} fields, the header has {}",
                    i + 1,
                    record.len(),
                    header.len()
                ));
            }
            let mut vars = Variables::default();
            for (name, value) in header.iter().zip(record) {
                vars.set(name.trim(), value);
            }
            Ok(vars)
        })
        .collect()
}

fn parse_csv(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    let mut line = 1;
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\n' => {
                    line += 1;
                    field.push(c);
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            ',' => record.push(std::mem::take(&mut field)),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                line += 1;
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            _ => field.push(c),
        }
    }
    if quoted {
        return Err(format!("unclosed quote (line {line})"));
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(v: &[Variables]) -> Vec<Vec<(String, String)>> {
        v.iter()
            .map(|vars| {
                vars.0
                    .iter()
                    .map(|v| (v.name.clone(), v.value.clone().unwrap_or_default()))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn csv() {
        let v = from_csv(
            "\u{feff}user, role\r\nalice,admin\r\n\"bob, jr\",\"say \"\"hi\"\"\nthere\"\n\n",
        )
        .unwrap();
        assert_eq!(
            rows(&v),
            [
                vec![
                    ("user".into(), "alice".into()),
                    ("role".into(), "admin".into())
                ],
                vec![
                    ("user".into(), "bob, jr".into()),
                    ("role".into(), "say \"hi\"\nthere".into())
                ],
            ]
        );
        assert!(
            from_csv("a,b\n1\n")
                .unwrap_err()
                .contains("row 1 has 1 fields")
        );
        assert!(
            from_csv("a,b\n\"1,2\n")
                .unwrap_err()
                .contains("unclosed quote")
        );
        assert!(from_csv("a,\n1,2\n").unwrap_err().contains("needs a name"));
        assert!(from_csv("").unwrap().is_empty());
    }

    #[test]
    fn json() {
        let v =
            from_json(r#"[{"user": "alice", "age": 30, "tags": ["a"]}, {"user": "bob"}]"#).unwrap();
        assert_eq!(
            rows(&v),
            [
                vec![
                    ("user".into(), "alice".into()),
                    ("age".into(), "30".into()),
                    ("tags".into(), "[\"a\"]".into())
                ],
                vec![("user".into(), "bob".into())],
            ]
        );
        assert!(from_json("{}").unwrap_err().contains("array of objects"));
        assert!(
            from_json("[1]")
                .unwrap_err()
                .contains("row 1: expected an object")
        );
    }
}

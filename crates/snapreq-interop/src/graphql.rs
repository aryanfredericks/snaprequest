//! GraphQL schema introspection (PLAN P10.3): the standard introspection query, and the
//! answer turned into SDL (the schema language) for reading.

use serde_json::Value;

use crate::{Error, Result};

/// The introspection query (the fields every GraphQL server supports, as GraphiQL sends them;
/// without `isRepeatable`, `specifiedByURL` and `includeDeprecated` on arguments, which older
/// servers reject).
pub const INTROSPECTION_QUERY: &str = r"query IntrospectionQuery {
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types { ...FullType }
    directives {
      name
      description
      locations
      args { ...InputValue }
    }
  }
}

fragment FullType on __Type {
  kind
  name
  description
  fields(includeDeprecated: true) {
    name
    description
    args { ...InputValue }
    type { ...TypeRef }
    isDeprecated
    deprecationReason
  }
  inputFields { ...InputValue }
  interfaces { ...TypeRef }
  enumValues(includeDeprecated: true) {
    name
    description
    isDeprecated
    deprecationReason
  }
  possibleTypes { ...TypeRef }
}

fragment InputValue on __InputValue {
  name
  description
  type { ...TypeRef }
  defaultValue
}

fragment TypeRef on __Type {
  kind
  name
  ofType {
    kind
    name
    ofType {
      kind
      name
      ofType {
        kind
        name
        ofType {
          kind
          name
          ofType {
            kind
            name
            ofType {
              kind
              name
              ofType { kind name }
            }
          }
        }
      }
    }
  }
}";

/// The `__schema` object of an introspection answer (`{"data": {"__schema": …}}`). GraphQL
/// errors in the answer are reported.
pub fn schema_of(answer: &Value) -> Result<&Value> {
    if let Some(schema) = answer.pointer("/data/__schema").filter(|s| s.is_object()) {
        return Ok(schema);
    }
    let errors: Vec<&str> = answer["errors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["message"].as_str())
        .collect();
    Err(Error::invalid(
        "GraphQL",
        if errors.is_empty() {
            "the answer has no `data.__schema` (is this a GraphQL endpoint with introspection on?)"
                .into()
        } else {
            format!("the server answered with errors: {}", errors.join("; "))
        },
    ))
}

/// A type reference as written in SDL: `[User!]!`.
pub fn type_ref(t: &Value) -> String {
    match t["kind"].as_str() {
        Some("NON_NULL") => format!("{}!", type_ref(&t["ofType"])),
        Some("LIST") => format!("[{}]", type_ref(&t["ofType"])),
        _ => t["name"].as_str().unwrap_or("?").to_owned(),
    }
}

fn description(out: &mut String, d: &Value, indent: &str) {
    if let Some(text) = d.as_str().filter(|t| !t.trim().is_empty()) {
        if text.contains('\n') || text.contains('"') {
            out.push_str(&format!("{indent}\"\"\"\n"));
            for line in text.lines() {
                out.push_str(&format!("{indent}{}\n", line.replace("\"\"\"", "\\\"\"\"")));
            }
            out.push_str(&format!("{indent}\"\"\"\n"));
        } else {
            out.push_str(&format!("{indent}\"{text}\"\n"));
        }
    }
}

fn args(list: &Value) -> String {
    let items: Vec<String> = list
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| {
            let mut s = format!(
                "{}: {}",
                a["name"].as_str().unwrap_or("?"),
                type_ref(&a["type"])
            );
            if let Some(d) = a["defaultValue"].as_str() {
                s.push_str(&format!(" = {d}"));
            }
            s
        })
        .collect();
    if items.is_empty() {
        String::new()
    } else {
        format!("({})", items.join(", "))
    }
}

fn deprecated(v: &Value) -> String {
    if v["isDeprecated"].as_bool() != Some(true) {
        return String::new();
    }
    match v["deprecationReason"].as_str() {
        Some(r) if r != "No longer supported" => format!(" @deprecated(reason: {r:?})"),
        _ => " @deprecated".into(),
    }
}

const BUILT_IN_SCALARS: [&str; 5] = ["String", "Int", "Float", "Boolean", "ID"];

/// The schema in SDL: root operation types (when not named `Query`, `Mutation` and
/// `Subscription`), then every type that is not built in, in the server's order.
pub fn to_sdl(schema: &Value) -> String {
    let mut out = String::new();
    let root = |key: &str| schema[key]["name"].as_str().map(str::to_owned);
    let roots = [
        ("query", root("queryType"), "Query"),
        ("mutation", root("mutationType"), "Mutation"),
        ("subscription", root("subscriptionType"), "Subscription"),
    ];
    if roots
        .iter()
        .any(|(_, name, usual)| name.as_deref().is_some_and(|n| n != *usual))
    {
        out.push_str("schema {\n");
        for (op, name, _) in &roots {
            if let Some(n) = name {
                out.push_str(&format!("  {op}: {n}\n"));
            }
        }
        out.push_str("}\n\n");
    }
    for t in schema["types"].as_array().into_iter().flatten() {
        let name = t["name"].as_str().unwrap_or_default();
        if name.starts_with("__") || BUILT_IN_SCALARS.contains(&name) {
            continue;
        }
        description(&mut out, &t["description"], "");
        let kind = t["kind"].as_str().unwrap_or_default();
        match kind {
            "SCALAR" => out.push_str(&format!("scalar {name}\n\n")),
            "UNION" => {
                let members: Vec<String> = t["possibleTypes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(type_ref)
                    .collect();
                out.push_str(&format!("union {name} = {}\n\n", members.join(" | ")));
            }
            "ENUM" => {
                out.push_str(&format!("enum {name} {{\n"));
                for v in t["enumValues"].as_array().into_iter().flatten() {
                    description(&mut out, &v["description"], "  ");
                    out.push_str(&format!(
                        "  {}{}\n",
                        v["name"].as_str().unwrap_or("?"),
                        deprecated(v)
                    ));
                }
                out.push_str("}\n\n");
            }
            "INPUT_OBJECT" => {
                out.push_str(&format!("input {name} {{\n"));
                for f in t["inputFields"].as_array().into_iter().flatten() {
                    description(&mut out, &f["description"], "  ");
                    let mut line = format!(
                        "  {}: {}",
                        f["name"].as_str().unwrap_or("?"),
                        type_ref(&f["type"])
                    );
                    if let Some(d) = f["defaultValue"].as_str() {
                        line.push_str(&format!(" = {d}"));
                    }
                    out.push_str(&line);
                    out.push('\n');
                }
                out.push_str("}\n\n");
            }
            _ => {
                let keyword = if kind == "INTERFACE" {
                    "interface"
                } else {
                    "type"
                };
                let interfaces: Vec<String> = t["interfaces"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(type_ref)
                    .collect();
                let implements = if interfaces.is_empty() {
                    String::new()
                } else {
                    format!(" implements {}", interfaces.join(" & "))
                };
                out.push_str(&format!("{keyword} {name}{implements} {{\n"));
                for f in t["fields"].as_array().into_iter().flatten() {
                    description(&mut out, &f["description"], "  ");
                    out.push_str(&format!(
                        "  {}{}: {}{}\n",
                        f["name"].as_str().unwrap_or("?"),
                        args(&f["args"]),
                        type_ref(&f["type"]),
                        deprecated(f)
                    ));
                }
                out.push_str("}\n\n");
            }
        }
    }
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn named(kind: &str, name: &str) -> Value {
        json!({ "kind": kind, "name": name, "ofType": null })
    }

    fn non_null(t: Value) -> Value {
        json!({ "kind": "NON_NULL", "name": null, "ofType": t })
    }

    #[test]
    fn schemas_become_sdl() {
        let schema = json!({
            "queryType": { "name": "Query" },
            "mutationType": null,
            "subscriptionType": null,
            "types": [
                { "kind": "OBJECT", "name": "Query", "description": null, "interfaces": [], "fields": [
                    { "name": "user", "description": "One user.", "isDeprecated": false,
                      "args": [{ "name": "id", "type": non_null(named("SCALAR", "ID")), "defaultValue": null }],
                      "type": named("OBJECT", "User") },
                    { "name": "users", "description": null, "isDeprecated": true, "deprecationReason": "use search",
                      "args": [{ "name": "first", "type": named("SCALAR", "Int"), "defaultValue": "10" }],
                      "type": non_null(json!({ "kind": "LIST", "name": null, "ofType": non_null(named("OBJECT", "User")) })) }
                ]},
                { "kind": "OBJECT", "name": "User", "description": null, "interfaces": [named("INTERFACE", "Node")], "fields": [
                    { "name": "id", "args": [], "type": non_null(named("SCALAR", "ID")), "isDeprecated": false },
                    { "name": "role", "args": [], "type": named("ENUM", "Role"), "isDeprecated": false }
                ]},
                { "kind": "INTERFACE", "name": "Node", "description": null, "interfaces": [], "fields": [
                    { "name": "id", "args": [], "type": non_null(named("SCALAR", "ID")), "isDeprecated": false }
                ]},
                { "kind": "ENUM", "name": "Role", "description": "What a user may do.", "enumValues": [
                    { "name": "ADMIN", "isDeprecated": false }, { "name": "READER", "isDeprecated": false }
                ]},
                { "kind": "INPUT_OBJECT", "name": "NewUser", "description": null, "inputFields": [
                    { "name": "name", "type": non_null(named("SCALAR", "String")), "defaultValue": null }
                ]},
                { "kind": "UNION", "name": "Result", "description": null, "possibleTypes": [named("OBJECT", "User")] },
                { "kind": "SCALAR", "name": "DateTime", "description": null },
                { "kind": "SCALAR", "name": "String", "description": null },
                { "kind": "OBJECT", "name": "__Type", "description": null, "fields": [] }
            ]
        });
        assert_eq!(
            to_sdl(&schema),
            r#"type Query {
  "One user."
  user(id: ID!): User
  users(first: Int = 10): [User!]! @deprecated(reason: "use search")
}

type User implements Node {
  id: ID!
  role: Role
}

interface Node {
  id: ID!
}

"What a user may do."
enum Role {
  ADMIN
  READER
}

input NewUser {
  name: String!
}

union Result = User

scalar DateTime
"#
        );
    }

    #[test]
    fn answers_without_a_schema_are_explained() {
        let err = schema_of(&json!({ "errors": [{ "message": "introspection is disabled" }] }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("introspection is disabled"), "{err}");
        assert!(schema_of(&json!({ "data": null })).is_err());
        let ok = json!({ "data": { "__schema": { "types": [] } } });
        assert!(schema_of(&ok).is_ok());
    }
}

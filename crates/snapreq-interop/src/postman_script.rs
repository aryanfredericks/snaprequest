//! Postman scripts to SnapRequest scripts (PLAN P9.4, ADR-032).
//!
//! The common `pm.*` calls (and the legacy `tests[...]`/`postman.*` API) are rewritten to the
//! `snapreq` API; `pm.expect` keeps its Chai chains, which `snapreq.expect` understands. A line
//! that still uses something without an equivalent (`pm.sendRequest`, `pm.cookies`, `require`, …)
//! is commented out when it is a statement on its own; otherwise the whole script is kept as a
//! comment. The result is checked to compile.

use std::sync::LazyLock;

use regex::Regex;

/// A translated script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Translated {
    pub code: String,
    /// Lines commented out because they could not be translated.
    pub skipped: usize,
    /// The whole script was kept as a comment.
    pub disabled: bool,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|e| panic!("bad built-in pattern {pattern}: {e}"))
}

/// `(pattern, replacement)`, applied in order.
static RULES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let status = |name: &str, check: &str| -> (String, String) {
        (
            format!(r"pm\.response\.to\.(?:be\.)?{name}\b"),
            format!("snapreq.expect(snapreq.response.status).{check}"),
        )
    };
    let mut rules: Vec<(String, String)> = vec![
        // Legacy `tests["name"] = expression;`
        (
            r#"(?m)^(\s*)tests\[\s*("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`[^`]*`)\s*\]\s*=\s*(.+?);?\s*$"#.into(),
            r#"${1}snapreq.test(${2}, () => { if (!(${3})) throw new Error("failed"); });"#.into(),
        ),
        (
            r"pm\.response\.to\.have\.status\(\s*(\d+)\s*\)".into(),
            "snapreq.expect(snapreq.response.status).to.equal($1)".into(),
        ),
        (
            r"pm\.response\.to\.not\.have\.status\(\s*(\d+)\s*\)".into(),
            "snapreq.expect(snapreq.response.status).to.not.equal($1)".into(),
        ),
        (
            r#"pm\.response\.to\.have\.header\(\s*([^(),]+?)\s*,\s*([^()]+?)\s*\)"#.into(),
            "snapreq.expect(snapreq.response.headers.get($1)).to.equal($2)".into(),
        ),
        (
            r"pm\.response\.to\.have\.header\(\s*([^()]+?)\s*\)".into(),
            "snapreq.expect(snapreq.response.headers.has($1)).to.be.true".into(),
        ),
        (
            r"pm\.response\.to\.not\.have\.header\(\s*([^()]+?)\s*\)".into(),
            "snapreq.expect(snapreq.response.headers.has($1)).to.be.false".into(),
        ),
        (
            r"pm\.response\.to\.have\.body\(\s*([^()]*?)\s*\)".into(),
            "snapreq.expect(snapreq.response.text()).to.equal($1)".into(),
        ),
        (
            r"pm\.response\.to\.(?:be\.)?json\b".into(),
            "snapreq.expect(() => snapreq.response.json()).to.not.throw()".into(),
        ),
    ];
    for (name, check) in [
        ("ok", "to.equal(200)"),
        ("success", "to.be.within(200, 299)"),
        ("info", "to.be.within(100, 199)"),
        ("redirection", "to.be.within(300, 399)"),
        ("clientError", "to.be.within(400, 499)"),
        ("serverError", "to.be.within(500, 599)"),
        ("error", "to.be.within(400, 599)"),
        ("accepted", "to.equal(202)"),
        ("badRequest", "to.equal(400)"),
        ("unauthorized", "to.equal(401)"),
        ("forbidden", "to.equal(403)"),
        ("notFound", "to.equal(404)"),
        ("rateLimited", "to.equal(429)"),
    ] {
        rules.push(status(name, check));
    }
    rules.extend(
        [
            (r"pm\.response\.json\(\)", "snapreq.response.json()"),
            (r"pm\.response\.text\(\)", "snapreq.response.text()"),
            (r"pm\.response\.code\b", "snapreq.response.status"),
            (r"pm\.response\.responseTime\b", "snapreq.response.time"),
            (r"pm\.response\.headers\.get\(", "snapreq.response.headers.get("),
            (r"pm\.response\.headers\.has\(", "snapreq.response.headers.has("),
            (
                r"pm\.(?:environment|collectionVariables|variables|globals|iterationData)\.(get|set|has)\(",
                "snapreq.variables.$1(",
            ),
            (
                r"pm\.request\.headers\.(?:add|upsert)\(\s*\{\s*key\s*:\s*(.+?)\s*,\s*value\s*:\s*(.+?)\s*,?\s*\}\s*\)",
                "snapreq.request.headers.set($1, $2)",
            ),
            (r"pm\.request\.headers\.remove\(", "snapreq.request.headers.remove("),
            (r"pm\.request\.headers\.get\(", "snapreq.request.headers.get("),
            (r"pm\.request\.method\b", "snapreq.request.method"),
            (r"pm\.request\.url\.toString\(\)", "snapreq.request.url"),
            (r"pm\.request\.body\.raw\b", "snapreq.request.body"),
            (r"pm\.test\(", "snapreq.test("),
            (r"pm\.expect\(", "snapreq.expect("),
            // Legacy API.
            (
                r"postman\.(?:setEnvironmentVariable|setGlobalVariable)\(",
                "snapreq.variables.set(",
            ),
            (
                r"postman\.(?:getEnvironmentVariable|getGlobalVariable)\(",
                "snapreq.variables.get(",
            ),
            (r"postman\.getResponseHeader\(", "snapreq.response.headers.get("),
            (r"\bresponseBody\b", "snapreq.response.text()"),
            (r"\bresponseCode\.code\b", "snapreq.response.status"),
            (r"\bresponseTime\b", "snapreq.response.time"),
            (
                r#"(^|[^.\w$])(?:environment|globals)\[\s*("[^"]*"|'[^']*')\s*\]"#,
                "${1}snapreq.variables.get(${2})",
            ),
            (
                r"(^|[^.\w$])(?:environment|globals)\.([A-Za-z_]\w*)",
                r#"${1}snapreq.variables.get("${2}")"#,
            ),
        ]
        .map(|(p, r)| (p.to_owned(), r.to_owned())),
    );
    rules
        .into_iter()
        .map(|(p, r)| (re(&p), &*Box::leak(r.into_boxed_str())))
        .collect()
});

/// Something left over that has no equivalent.
static LEFTOVER: LazyLock<Regex> =
    LazyLock::new(|| re(r"(^|[^.\w$])(pm|postman)\.|\brequire\s*\(|(^|[^.\w$])tests\["));

/// Whether brackets balance on one line (strings ignored), so commenting it out is safe.
fn balanced(line: &str) -> bool {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in line.chars() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => quote = Some(c),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    depth == 0 && quote.is_none()
}

fn as_comment(src: &str, why: &str) -> String {
    let mut out = format!("// {why}\n");
    for line in src.lines() {
        out.push_str("// ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Translates one Postman script.
pub(crate) fn translate(src: &str) -> Translated {
    let mut code = src.to_owned();
    for (pattern, replacement) in RULES.iter() {
        code = pattern.replace_all(&code, *replacement).into_owned();
    }
    let mut skipped = 0;
    let mut lines = Vec::new();
    let mut whole = false;
    for line in code.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("//") && LEFTOVER.is_match(line) {
            if balanced(line) {
                skipped += 1;
                let indent = &line[..line.len() - trimmed.len()];
                lines.push(format!("{indent}// not translated: {trimmed}"));
                continue;
            }
            whole = true;
        }
        lines.push(line.to_owned());
    }
    let code = lines.join("\n");
    if whole || snapreq_script::check_syntax(&code).is_err() {
        return Translated {
            code: as_comment(
                src,
                "Imported from Postman; this script could not be translated, edit it to use the snapreq API.",
            ),
            skipped: 0,
            disabled: true,
        };
    }
    Translated {
        code: format!("{code}\n"),
        skipped,
        disabled: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_calls_are_rewritten() {
        let t = translate(
            r#"pm.test("ok", function () {
    pm.response.to.have.status(200);
    pm.response.to.be.ok;
    const body = pm.response.json();
    pm.expect(body.id).to.eql(7);
    pm.environment.set("id", body.id);
    pm.expect(pm.response.responseTime).to.be.below(500);
    pm.response.to.have.header("Content-Type");
});"#,
        );
        assert!(!t.disabled && t.skipped == 0, "{t:?}");
        assert_eq!(
            t.code,
            r#"snapreq.test("ok", function () {
    snapreq.expect(snapreq.response.status).to.equal(200);
    snapreq.expect(snapreq.response.status).to.equal(200);
    const body = snapreq.response.json();
    snapreq.expect(body.id).to.eql(7);
    snapreq.variables.set("id", body.id);
    snapreq.expect(snapreq.response.time).to.be.below(500);
    snapreq.expect(snapreq.response.headers.has("Content-Type")).to.be.true;
});
"#
        );
    }

    #[test]
    fn legacy_tests_and_globals() {
        let t = translate(
            "var data = JSON.parse(responseBody);\ntests[\"has id\"] = data.id > 0;\npostman.setEnvironmentVariable(\"id\", data.id);\nvar host = environment.host;",
        );
        assert!(!t.disabled, "{t:?}");
        assert!(t.code.contains("JSON.parse(snapreq.response.text())"));
        assert!(t.code.contains(
            "snapreq.test(\"has id\", () => { if (!(data.id > 0)) throw new Error(\"failed\"); });"
        ));
        assert!(t.code.contains("snapreq.variables.set(\"id\", data.id)"));
        assert!(
            t.code
                .contains("var host = snapreq.variables.get(\"host\");")
        );
    }

    #[test]
    fn untranslatable_lines_are_commented_out_or_the_whole_script() {
        let t = translate("pm.cookies.clear();\npm.test(\"a\", () => {});");
        assert_eq!(t.skipped, 1);
        assert!(
            t.code
                .starts_with("// not translated: pm.cookies.clear();\nsnapreq.test(")
        );
        let t = translate(
            "pm.sendRequest(\"https://x\", function (err, res) {\n  pm.environment.set(\"t\", res.json().t);\n});",
        );
        assert!(t.disabled, "{t:?}");
        assert!(t.code.lines().skip(1).all(|l| l.starts_with("// ")));
        let t = translate("const x = require('lodash');");
        assert_eq!(t.skipped, 1);
    }
}

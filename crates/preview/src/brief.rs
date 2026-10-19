//! What a model is told of a spec to write the behaviour of the back end
//! simulated from it: every operation in a line, then the schemas of those
//! the person's request names or the page called, with the components they
//! name, within a size a model reads at once.

use std::collections::BTreeSet;
use std::fmt::Write;

use serde_json::Value;

use crate::spec::Spec;

/// How much of the schemas is told, at most, in characters.
const LARGEST: usize = 120_000;

/// How many operations are told with their schemas, at most.
const MOST: usize = 40;

/// What a model is told of `spec` for `asked`, the person's request, the
/// page having called `called` (`GET /v1/orders/?limit=10`, the latest
/// last), and the program handling `handled`.
#[must_use]
pub fn brief(spec: &Spec, asked: &str, called: &[String], handled: &[String]) -> String {
    let mut text = String::from("Operations of the spec, a method, a path and what it does:\n");
    for operation in &spec.operations {
        let _ = write!(text, "{} {}", operation.method, operation.path);
        if !operation.summary.is_empty() {
            let _ = write!(text, ": {}", operation.summary);
        }
        text.push('\n');
    }
    let mut concerned: Vec<usize> = Vec::new();
    let mut add = |index: Option<usize>| {
        if let Some(index) = index
            && !concerned.contains(&index)
            && concerned.len() < MOST
        {
            concerned.push(index);
        }
    };
    for line in handled.iter().chain(called.iter().rev()) {
        let (method, path) = line.split_once(' ').unwrap_or(("GET", line));
        add(spec.find(&method.to_uppercase(), path.trim()));
    }
    let words = words(asked);
    for (index, operation) in spec.operations.iter().enumerate() {
        let named = operation
            .path
            .split(['/', '-', '_'])
            .filter(|p| p.len() >= 4 && !p.starts_with('{'))
            .any(|part| {
                let part = part.to_lowercase();
                words
                    .iter()
                    .any(|w| part.starts_with(w.as_str()) || w.starts_with(part.as_str()))
            });
        if named {
            add(Some(index));
        }
    }
    text.push_str(
        "\nThe operations concerned, their request's schema and their answers' by status:\n",
    );
    let mut references: Vec<String> = Vec::new();
    for index in concerned {
        let operation = &spec.operations[index];
        let _ = writeln!(text, "\n{} {}", operation.method, operation.path);
        if let Some(schema) = spec.request_schema(index) {
            let _ = writeln!(text, "request: {schema}");
            named_in(schema, &mut references);
        }
        for status in &operation.statuses {
            if let Some(schema) = spec.schema(index, status) {
                let _ = writeln!(text, "{status}: {schema}");
                named_in(schema, &mut references);
            } else {
                let _ = writeln!(text, "{status}: no body");
            }
        }
    }
    text.push_str("\nThe schemas they name:\n");
    let mut told: BTreeSet<String> = BTreeSet::new();
    let mut at = 0;
    while at < references.len() {
        let reference = references[at].clone();
        at += 1;
        if !told.insert(reference.clone()) {
            continue;
        }
        let Some(schema) = spec.named(&reference) else {
            continue;
        };
        let line = format!("{reference}: {schema}\n");
        if text.len() + line.len() > LARGEST {
            text.push_str("(the others left out, for size)\n");
            break;
        }
        text.push_str(&line);
        named_in(schema, &mut references);
    }
    text
}

/// The words of a request a path may hold: `checkout` and `order` of
/// "when a checkout is confirmed, make its order".
fn words(asked: &str) -> Vec<String> {
    asked
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4)
        .map(str::to_lowercase)
        .collect()
}

/// The `$ref`s a schema names, added to `references` in turn.
fn named_in(schema: &Value, references: &mut Vec<String>) {
    match schema {
        Value::Object(fields) => {
            for (name, value) in fields {
                match (name.as_str(), value) {
                    ("$ref", Value::String(reference)) => references.push(reference.clone()),
                    _ => named_in(value, references),
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                named_in(item, references);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_brief_tells_every_operation_and_the_schemas_of_those_concerned() {
        let spec = Spec::read(
            r##"{"paths": {
                "/v1/checkouts/{id}/confirm": {"post": {"summary": "Confirm", "requestBody": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Confirm"}}}},
                    "responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Checkout"}}}}, "404": {"description": "none"}}}},
                "/v1/orders/": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Orders"}}}}}}},
                "/v1/metrics": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Metrics"}}}}}}}},
              "components": {"schemas": {
                "Confirm": {"type": "object"},
                "Checkout": {"type": "object", "properties": {"customer": {"$ref": "#/components/schemas/Customer"}}},
                "Customer": {"type": "object"},
                "Orders": {"type": "object"},
                "Metrics": {"type": "object"}}}}"##,
        )
        .unwrap();
        let told = brief(
            &spec,
            "when a checkout is confirmed",
            &["GET /v1/orders/?limit=1".to_owned()],
            &[],
        );
        assert!(told.contains("POST /v1/checkouts/{id}/confirm: Confirm\n"));
        assert!(told.contains("GET /v1/metrics\n"));
        assert!(told.contains("request: {\"$ref\":\"#/components/schemas/Confirm\"}"));
        assert!(told.contains("404: no body"));
        // Named through another, told once.
        assert_eq!(told.matches("#/components/schemas/Customer: ").count(), 1);
        assert!(told.contains("#/components/schemas/Orders: "));
        assert!(!told.contains("#/components/schemas/Metrics: "));
    }
}

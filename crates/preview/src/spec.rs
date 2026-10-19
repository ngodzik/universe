//! An OpenAPI spec as the simulated back end reads it: each operation's
//! method and path, how the path a request asks for leads to it, and what
//! it answers, by status, with the schemas it names.

use serde_json::Value;
use yaml_rust2::{Yaml, YamlLoader};

use crate::error::PreviewError;
use crate::made::LISTS;

/// A spec, read for simulating the back end it describes.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    /// The document, its `$ref`s as written.
    doc: Value,
    /// The path its servers' address starts with, which the operations'
    /// paths are under: `/api/v1`, or nothing.
    base: Vec<String>,
    /// Its operations, in the spec's order.
    pub operations: Vec<Operation>,
}

/// An operation of a spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    /// `GET`, `POST`…
    pub method: String,
    /// Its path, parameters in braces: `/v1/products/{id}`.
    pub path: String,
    /// What it does, in a line, as the spec says.
    pub summary: String,
    /// The statuses it may answer, in the spec's order: `200`, `404`.
    pub statuses: Vec<String>,
    /// Its path's parts, `None` for a parameter.
    parts: Vec<Option<String>>,
}

impl Spec {
    /// Reads a spec from its text, JSON or YAML.
    ///
    /// # Errors
    ///
    /// [`PreviewError::SpecJson`] or [`PreviewError::SpecYaml`] when it is
    /// neither, [`PreviewError::NoPaths`] when it has no paths.
    pub fn read(text: &str) -> Result<Self, PreviewError> {
        let doc = if text.trim_start().starts_with('{') {
            serde_json::from_str(text).map_err(PreviewError::SpecJson)?
        } else {
            let docs = YamlLoader::load_from_str(text).map_err(PreviewError::SpecYaml)?;
            docs.into_iter().next().map(json_of).unwrap_or_default()
        };
        let Some(paths) = doc["paths"].as_object() else {
            return Err(PreviewError::NoPaths);
        };
        let mut operations = Vec::new();
        for (path, item) in paths {
            let Some(item) = item.as_object() else {
                continue;
            };
            for (method, operation) in item {
                let method = method.to_uppercase();
                if !matches!(
                    method.as_str(),
                    "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
                ) {
                    continue;
                }
                let statuses = operation["responses"]
                    .as_object()
                    .map(|r| r.keys().cloned().collect())
                    .unwrap_or_default();
                operations.push(Operation {
                    method,
                    path: path.clone(),
                    summary: operation["summary"]
                        .as_str()
                        .unwrap_or_default()
                        .trim()
                        .to_owned(),
                    statuses,
                    parts: path
                        .split('/')
                        .filter(|p| !p.is_empty())
                        .map(|p| (!p.starts_with('{')).then(|| p.to_owned()))
                        .collect(),
                });
            }
        }
        let base = doc["servers"][0]["url"]
            .as_str()
            .map(|url| {
                let path = url
                    .split_once("://")
                    .map_or(url, |(_, rest)| rest.find('/').map_or("", |i| &rest[i..]));
                path.split('/')
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            doc,
            base,
            operations,
        })
    }

    /// The operation a request for `path` with `method` asks for: of those
    /// whose path it matches, the one that names most of it. The servers'
    /// own path may be in front, or not.
    #[must_use]
    pub fn find(&self, method: &str, path: &str) -> Option<usize> {
        let path = path.split(['?', '#']).next().unwrap_or(path);
        let asked: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let under_base = asked.len() >= self.base.len()
            && !self.base.is_empty()
            && asked.iter().zip(&self.base).all(|(a, b)| a == b);
        let ways: Vec<&[&str]> = if under_base {
            vec![&asked[self.base.len()..], &asked[..]]
        } else {
            vec![&asked[..]]
        };
        ways.into_iter().find_map(|asked| {
            self.operations
                .iter()
                .enumerate()
                .filter(|(_, o)| o.method == method && o.parts.len() == asked.len())
                .filter(|(_, o)| {
                    o.parts
                        .iter()
                        .zip(asked)
                        .all(|(part, a)| part.as_deref().is_none_or(|p| p == *a))
                })
                .max_by_key(|(i, o)| {
                    (
                        o.parts.iter().filter(|p| p.is_some()).count(),
                        usize::MAX - i,
                    )
                })
                .map(|(i, _)| i)
        })
    }

    /// The parts of `path` that operation `index` reads, as asked, decoded:
    /// those under the servers' own path, `["v1", "products", "42"]` for
    /// `/api/v1/products/42?x=1`.
    pub(crate) fn bound(&self, index: usize, path: &str) -> Vec<String> {
        let path = path.split(['?', '#']).next().unwrap_or(path);
        let asked: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let length = self.operations.get(index).map_or(0, |o| o.parts.len());
        asked[asked.len().saturating_sub(length)..]
            .iter()
            .map(|part| decoded(part))
            .collect()
    }

    /// The schema operation `index` answers with, for `status`: of its JSON
    /// body, if it has one.
    pub(crate) fn schema(&self, index: usize, status: &str) -> Option<&Value> {
        let operation = self.operations.get(index)?;
        let response = self.resolve(
            &self.doc["paths"][&operation.path][operation.method.to_lowercase()]["responses"]
                [status],
        );
        let content = response["content"].as_object()?;
        content
            .iter()
            .find(|(media, _)| media.contains("json"))
            .or_else(|| content.iter().next())
            .map(|(_, media)| &media["schema"])
            .filter(|s| !s.is_null())
    }

    /// The schema of the JSON body operation `index` is sent, if it takes
    /// one.
    pub(crate) fn request_schema(&self, index: usize) -> Option<&Value> {
        let operation = self.operations.get(index)?;
        let body = self.resolve(
            &self.doc["paths"][&operation.path][operation.method.to_lowercase()]["requestBody"],
        );
        let content = body["content"].as_object()?;
        content
            .iter()
            .find(|(media, _)| media.contains("json"))
            .map(|(_, media)| &media["schema"])
            .filter(|s| !s.is_null())
    }

    /// What checks a body operation `index` answers with `status` against
    /// its schema, the spec's components with it: for a spec of OpenAPI
    /// 3.1 only, whose schemas are JSON Schema's; none without a schema.
    pub(crate) fn validator(&self, index: usize, status: &str) -> Option<jsonschema::Validator> {
        let version = self.doc["openapi"].as_str().unwrap_or_default();
        if !version.starts_with("3.1") {
            return None;
        }
        let schema = self.schema(index, status)?;
        let whole = serde_json::json!({
            "components": self.doc["components"],
            "allOf": [schema],
        });
        jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(&whole)
            .ok()
    }

    /// The schema operation `index` answers with when it succeeds.
    pub(crate) fn success_schema(&self, index: usize) -> Option<&Value> {
        let operation = self.operations.get(index)?;
        let status = operation
            .statuses
            .iter()
            .find(|s| s.starts_with('2'))
            .map_or("200", String::as_str);
        self.schema(index, status)
    }

    /// The schema of the items a list operation answers with when it
    /// succeeds: of the answer itself when it is a list, else of its field
    /// that holds a page of them, else of its only list.
    pub(crate) fn listed(&self, index: usize) -> Option<&Value> {
        let answer = self.resolve(self.success_schema(index)?);
        if answer["type"] == "array" {
            return Some(&answer["items"]).filter(|items| !items.is_null());
        }
        let lists: Vec<(&String, &Value)> = answer["properties"]
            .as_object()?
            .iter()
            .filter(|(_, p)| self.resolve(p)["type"] == "array")
            .collect();
        let (_, list) = lists
            .iter()
            .find(|(name, _)| LISTS.contains(&name.as_str()))
            .or_else(|| (lists.len() == 1).then(|| &lists[0]))?;
        Some(&self.resolve(list)["items"]).filter(|items| !items.is_null())
    }

    /// Whether a schema's objects have an id: theirs, or those of each of
    /// its kinds.
    pub(crate) fn names_items(&self, schema: &Value) -> bool {
        self.within(schema, 0, &mut |schema| {
            schema["properties"].get("id").is_some()
        })
    }

    /// The values a field of a schema's objects takes, as its enumerations
    /// say: `canceled` among a subscription's `status`.
    pub(crate) fn enumerated(&self, schema: &Value, field: &str) -> Vec<String> {
        let mut values = Vec::new();
        self.within(schema, 0, &mut |schema| {
            if let Some(property) = schema["properties"].get(field) {
                self.within(property, 0, &mut |kind| {
                    let listed = kind["enum"].as_array().into_iter().flatten();
                    let constant = kind.get("const").into_iter();
                    values.extend(
                        listed
                            .chain(constant)
                            .filter_map(Value::as_str)
                            .map(str::to_owned),
                    );
                    false
                });
            }
            false
        });
        values
    }

    /// Whether `found` says so of `schema` or of one of the kinds it is
    /// made of, through `anyOf`, `oneOf` and `allOf`, a few levels down.
    fn within(&self, schema: &Value, depth: usize, found: &mut dyn FnMut(&Value) -> bool) -> bool {
        if depth > 4 {
            return false;
        }
        let schema = self.resolve(schema);
        if found(schema) {
            return true;
        }
        ["anyOf", "oneOf", "allOf"].iter().any(|kinds| {
            schema[*kinds]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| self.within(kind, depth + 1, found)))
        })
    }

    /// What `node` stands for: itself, or what its `$ref` names in the spec,
    /// followed a few times.
    pub(crate) fn resolve<'a>(&'a self, node: &'a Value) -> &'a Value {
        let mut node = node;
        for _ in 0..8 {
            let Some(reference) = node["$ref"].as_str() else {
                return node;
            };
            let Some(pointer) = reference.strip_prefix('#') else {
                return node;
            };
            match self.doc.pointer(pointer) {
                Some(target) => node = target,
                None => return node,
            }
        }
        node
    }

    /// The schema a `$ref` names: `#/components/schemas/Order`.
    pub(crate) fn named(&self, reference: &str) -> Option<&Value> {
        self.doc.pointer(reference.strip_prefix('#')?)
    }

    /// The name a `$ref` gives its schema, for knowing a schema met again.
    pub(crate) fn reference(node: &Value) -> Option<&str> {
        node["$ref"].as_str()
    }
}

impl Operation {
    /// Its path's parts, `None` for a parameter.
    pub(crate) fn parts(&self) -> &[Option<String>] {
        &self.parts
    }
}

/// A part of an address as it reads, its escapes undone: `a b` for
/// `a%20b`.
pub(crate) fn decoded(part: &str) -> String {
    let bytes = part.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        let escaped = (byte == b'%')
            .then(|| part.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(value) = escaped {
            out.push(value);
            i += 3;
        } else {
            out.push(byte);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A YAML document as JSON.
fn json_of(yaml: Yaml) -> Value {
    match yaml {
        Yaml::Real(r) => r
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::String(r), Value::Number),
        Yaml::Integer(i) => Value::from(i),
        Yaml::String(s) => Value::String(s),
        Yaml::Boolean(b) => Value::Bool(b),
        Yaml::Array(items) => Value::Array(items.into_iter().map(json_of).collect()),
        Yaml::Hash(hash) => Value::Object(
            hash.into_iter()
                .map(|(k, v)| {
                    let key = match k {
                        Yaml::String(s) => s,
                        Yaml::Integer(i) => i.to_string(),
                        Yaml::Real(r) => r,
                        Yaml::Boolean(b) => b.to_string(),
                        _ => String::new(),
                    };
                    (key, json_of(v))
                })
                .collect(),
        ),
        Yaml::Alias(_) | Yaml::Null | Yaml::BadValue => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_finds_the_operation_that_names_most_of_its_path() {
        let spec = Spec::read(
            "openapi: 3.1.0\nservers:\n  - url: https://shop.example.com/api\npaths:\n  /products/{id}:\n    get:\n      summary: Get Product\n      responses:\n        '200': {description: ok}\n        '404': {description: none}\n  /products/new:\n    get: {}\n  /products/:\n    post: {}\n",
        )
        .unwrap();
        let found = |method: &str, path: &str| {
            spec.find(method, path)
                .map(|i| spec.operations[i].path.as_str())
        };
        assert_eq!(found("GET", "/products/42"), Some("/products/{id}"));
        assert_eq!(found("GET", "/products/new?x=1"), Some("/products/new"));
        assert_eq!(found("GET", "/api/products/42"), Some("/products/{id}"));
        assert_eq!(found("POST", "/products"), Some("/products/"));
        assert_eq!(found("DELETE", "/products/42"), None);
        let get = &spec.operations[spec.find("GET", "/products/1").unwrap()];
        assert_eq!(get.statuses, ["200", "404"]);
        assert_eq!(get.summary, "Get Product");
        assert!(Spec::read("{\"openapi\": \"3.1.0\"}").is_err());
        let item = spec.find("GET", "/api/products/a%20b").unwrap();
        assert_eq!(
            spec.bound(item, "/api/products/a%20b?x=1"),
            ["products", "a b"]
        );
        assert_eq!(decoded("x+y%2Fz%"), "x+y/z%");
    }

    #[test]
    fn a_response_schema_through_its_refs() {
        let spec = Spec::read(
            r##"{"paths": {"/me": {"get": {"responses": {"200": {"$ref": "#/components/responses/Me"}}}}},
                "components": {"responses": {"Me": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/User"}}}}},
                               "schemas": {"User": {"type": "object", "properties": {"name": {"type": "string"}}}}}}"##,
        )
        .unwrap();
        let schema = spec.schema(0, "200").unwrap();
        assert_eq!(Spec::reference(schema), Some("#/components/schemas/User"));
        assert_eq!(spec.resolve(schema)["properties"]["name"]["type"], "string");
        assert!(spec.schema(0, "404").is_none());
    }
}

//! What the simulated back end answers, made from a schema, the same for
//! the same schema and place: values that look like what the field names
//! (an id like an id, a date like a date, an email like an email), each
//! item of a list its own, every value of an enumeration in a list of them
//! (a session's permissions all granted), flags off, a date of something
//! that may not have happened (`canceled_at`) not set.
//!
//! A [`Tone`] bends it to a state worth seeing: lists empty, rights
//! withheld, every flag on.

use serde_json::{Map, Value, json};

use crate::spec::Spec;

/// How deep it goes into nested objects and lists before it makes them
/// empty: a list in an object in a list… ten levels down.
const DEEPEST: usize = 10;

/// How many items a list has.
const ITEMS: usize = 3;

/// A state the answers are bent to, beyond what the spec says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Tone {
    /// Every list empty, its counts at nothing.
    pub empty: bool,
    /// No permission granted, no role above the least.
    pub no_rights: bool,
    /// Every flag of a feature on.
    pub flags_on: bool,
}

/// The fields that hold a page of a list.
const LISTS: [&str; 8] = [
    "items", "data", "results", "records", "entries", "nodes", "rows", "edges",
];

/// The fields that count a list.
const COUNTS: [&str; 7] = [
    "total_count",
    "total",
    "count",
    "total_items",
    "max_page",
    "total_pages",
    "totalcount",
];

/// People, for the names and emails of a list's items.
const PEOPLE: [(&str, &str); 4] = [
    ("Ada Lovelace", "ada@example.com"),
    ("Grace Hopper", "grace@example.com"),
    ("Alan Turing", "alan@example.com"),
    ("Edsger Dijkstra", "edsger@example.com"),
];

/// A picture any page may show, its policy of content allowing `data:`.
const PICTURE: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 40 40'><rect width='40' height='40' rx='20' fill='#6366f1'/><text x='20' y='26' font-size='15' text-anchor='middle' fill='white' font-family='sans-serif'>U</text></svg>";

/// Strings tried, in turn, for one whose pattern the made one does not
/// match: a decimal, a number, words.
const PATTERNED: [&str; 12] = [
    "12.50", "0.25", "12", "1", "demo", "Demo", "DEMO", "demo-1", "demo_1", "abc123", "A1", "a",
];

/// A body made from `schema`, `key` naming where it is asked for (the
/// operation and its status), so that two places differ.
#[must_use]
pub(crate) fn body(spec: &Spec, schema: &Value, key: &str, tone: Tone) -> Value {
    let mut maker = Maker {
        spec,
        tone,
        stack: Vec::new(),
    };
    maker.make(schema, key, "", 0)
}

struct Maker<'a> {
    spec: &'a Spec,
    tone: Tone,
    /// The schemas named on the way down, so that one that holds itself
    /// stops.
    stack: Vec<String>,
}

impl Maker<'_> {
    fn make(&mut self, schema: &Value, key: &str, field: &str, depth: usize) -> Value {
        // Objects and lists are made empty past `DEEPEST`; this stops what
        // nests otherwise.
        if depth > DEEPEST * 2 {
            return Value::Null;
        }
        let named = Spec::reference(schema).map(str::to_owned);
        if let Some(name) = &named {
            if self.stack.iter().filter(|n| *n == name).count() >= 2 {
                return Value::Null;
            }
            self.stack.push(name.clone());
        }
        let schema = self.spec.resolve(schema).clone();
        let made = self.made(&schema, key, field, depth);
        if named.is_some() {
            self.stack.pop();
        }
        made
    }

    fn made(&mut self, schema: &Value, key: &str, field: &str, depth: usize) -> Value {
        let lower = field.to_lowercase();
        if let Some(value) = schema.get("const") {
            return value.clone();
        }
        if let Some(choices) = schema["enum"].as_array() {
            if self.tone.no_rights && lower == "role" {
                return choices.last().cloned().unwrap_or(Value::Null);
            }
            return choices.first().cloned().unwrap_or(Value::Null);
        }
        for choice in ["anyOf", "oneOf"] {
            if let Some(options) = schema[choice].as_array() {
                let nullable = options
                    .iter()
                    .any(|o| self.spec.resolve(o)["type"] == "null");
                // What may not have happened has not: a date of ending,
                // of cancelling.
                let happened =
                    ["created_at", "modified_at", "updated_at"].contains(&lower.as_str());
                if nullable && lower.ends_with("_at") && !happened {
                    return Value::Null;
                }
                // The kind a discriminator names, its property saying so.
                if let Some((property, named, option)) = self.discriminated(schema, options) {
                    let mut made = self.make(&option, key, field, depth);
                    if let (Value::Object(fields), Some(named)) = (&mut made, named) {
                        fields.insert(property, Value::String(named));
                    }
                    return made;
                }
                // The first that is not null, and not one this is already
                // within: a filter of filters ends with a clause.
                let candidates: Vec<&Value> = options
                    .iter()
                    .filter(|o| self.spec.resolve(o)["type"] != "null")
                    .collect();
                let within = |o: &&Value| {
                    Spec::reference(o).is_some_and(|r| self.stack.iter().any(|n| n == r))
                };
                let Some(chosen) = candidates
                    .iter()
                    .find(|o| !within(o))
                    .or_else(|| candidates.first())
                else {
                    return Value::Null;
                };
                return self.make(chosen, key, field, depth);
            }
        }
        if let Some(parts) = schema["allOf"].as_array() {
            let mut whole = Map::new();
            for part in parts {
                if let Value::Object(fields) = self.make(part, key, field, depth) {
                    whole.extend(fields);
                }
            }
            return Value::Object(whole);
        }
        let kind = match &schema["type"] {
            Value::String(kind) => kind.clone(),
            // `["string", "null"]`: the first that is not null.
            Value::Array(kinds) => kinds
                .iter()
                .filter_map(Value::as_str)
                .find(|k| *k != "null")
                .unwrap_or("null")
                .to_owned(),
            _ if schema.get("properties").is_some() => "object".to_owned(),
            _ if schema.get("items").is_some() => "array".to_owned(),
            _ => String::new(),
        };
        if let Some(example) = schema
            .get("example")
            .filter(|_| kind != "object" && kind != "array")
        {
            return example.clone();
        }
        if let Some(example) = schema["examples"].as_array().and_then(|e| e.first())
            && kind != "object"
            && kind != "array"
        {
            return example.clone();
        }
        match kind.as_str() {
            "object" if depth > DEEPEST => Value::Object(Map::new()),
            "object" => {
                let mut fields = Map::new();
                if let Some(properties) = schema["properties"].as_object() {
                    for (name, property) in properties {
                        let value = self.make(property, &format!("{key}.{name}"), name, depth + 1);
                        fields.insert(name.clone(), value);
                    }
                }
                Value::Object(fields)
            }
            "array" => self.list(schema, key, &lower, depth),
            "integer" => within_bounds(schema, self.integer(key, &lower)),
            "number" => within_bounds(schema, json!(f64::from(seed(key) % 10_000) / 100.0)),
            "boolean" => Value::Bool(self.boolean(&lower)),
            "string" => fitted(schema, self.string(schema, key, &lower)),
            _ => Value::Null,
        }
    }

    /// The option a discriminator names first: its property, the name
    /// its mapping gives it, and the option. OpenAPI's way of saying which
    /// of several kinds a body is; without a mapping, the option says it
    /// with a constant of its own.
    fn discriminated(
        &self,
        schema: &Value,
        options: &[Value],
    ) -> Option<(String, Option<String>, Value)> {
        let discriminator = &schema["discriminator"];
        let property = discriminator["propertyName"].as_str()?.to_owned();
        let first = options.first()?;
        let Some(mapping) = discriminator["mapping"].as_object() else {
            return Some((property, None, first.clone()));
        };
        // The first option in the spec's order, by the name it maps from.
        let named = options.iter().find_map(|option| {
            let reference = Spec::reference(option)?;
            mapping
                .iter()
                .find(|(_, r)| r.as_str() == Some(reference))
                .map(|(name, _)| (name.clone(), option.clone()))
        });
        match named {
            Some((name, option)) => Some((property, Some(name), option)),
            None => {
                let (name, reference) = mapping.iter().next()?;
                Some((
                    property,
                    Some(name.clone()),
                    json!({ "$ref": reference.as_str()? }),
                ))
            }
        }
    }

    fn list(&mut self, schema: &Value, key: &str, field: &str, depth: usize) -> Value {
        // A tuple, `[number, format]`: each place its own, as the spec's
        // example has it when it has one.
        if let Some(places) = schema["prefixItems"]
            .as_array()
            .or_else(|| schema["items"].as_array())
        {
            if let Some(example) = schema["examples"]
                .as_array()
                .and_then(|e| e.first())
                .filter(|e| e.as_array().is_some_and(|e| e.len() == places.len()))
            {
                return example.clone();
            }
            let made = places
                .iter()
                .enumerate()
                .map(|(i, place)| self.make(place, &format!("{key}[{i}]"), "", depth + 1))
                .collect();
            return Value::Array(made);
        }
        let fewest = schema["minItems"]
            .as_u64()
            .map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
        let most = schema["maxItems"]
            .as_u64()
            .map_or(usize::MAX, |n| usize::try_from(n).unwrap_or(usize::MAX));
        if fewest == 0 {
            if self.tone.empty && LISTS.contains(&field) {
                return Value::Array(Vec::new());
            }
            let rights = ["permission", "scope", "right", "grant", "capabilit"]
                .iter()
                .any(|w| field.contains(w));
            if (self.tone.no_rights && rights) || depth > DEEPEST {
                return Value::Array(Vec::new());
            }
        }
        let items = &schema["items"];
        let resolved = self.spec.resolve(items);
        // A list of an enumeration's values holds them all: a session's
        // permissions, all granted.
        if let Some(choices) = resolved["enum"].as_array() {
            return Value::Array(choices.iter().take(most).cloned().collect());
        }
        let count = ITEMS.min(most).max(fewest);
        let mut made: Vec<Value> = Vec::new();
        for i in 0..count {
            let item_key = format!("{key}[{i}]");
            let value = distinct(self.make(items, &item_key, "", depth + 1), i);
            if schema["uniqueItems"] == true && made.contains(&value) && made.len() >= fewest {
                continue;
            }
            made.push(value);
        }
        // Items that could not be made, a schema within itself: none.
        if fewest == 0 && made.iter().all(Value::is_null) {
            made.clear();
        }
        Value::Array(made)
    }

    fn integer(&self, key: &str, field: &str) -> Value {
        if self.tone.empty && COUNTS.contains(&field) {
            return json!(0);
        }
        if ["amount", "price", "revenue", "balance", "cost"]
            .iter()
            .any(|w| field.contains(w))
        {
            return json!(1900 + seed(key) % 8000);
        }
        match field {
            "page" | "max_page" | "total_pages" => json!(1),
            "total_count" | "count" | "total" | "total_items" => json!(ITEMS),
            _ => json!(seed(key) % 100),
        }
    }

    /// An id the same wherever the same thing is: the first organization's
    /// in a list of them, in a session's and as an order's
    /// `organization_id`. It names the entity of the schema it is in, or
    /// the one its field names, and its place in a list.
    fn id(&self, key: &str, field: &str) -> String {
        let entity = if field == "id" {
            self.stack.last().map(|name| entity(name))
        } else {
            field.strip_suffix("_id").map(|name| name.replace('_', ""))
        };
        match entity {
            Some(entity) if !entity.is_empty() => {
                let place = if field == "id" { place(key) } else { 0 };
                uuid(&format!("{entity}#{place}"))
            }
            _ => uuid(key),
        }
    }

    fn boolean(&self, field: &str) -> bool {
        if self.tone.no_rights
            && ["is_admin", "admin", "is_owner", "is_superuser", "is_staff"].contains(&field)
        {
            return false;
        }
        if self.tone.flags_on
            && (field.ends_with("_enabled") || field.contains("feature") || field.contains("flag"))
        {
            return true;
        }
        false
    }

    fn string(&self, schema: &Value, key: &str, field: &str) -> Value {
        let format = schema["format"].as_str().unwrap_or_default();
        let id = format == "uuid" || field == "id" || field.ends_with("_id");
        if id {
            return Value::String(self.id(key, field));
        }
        if format == "date-time" || field.ends_with("_at") {
            return Value::String(format!(
                "2026-09-{:02}T{:02}:00:00Z",
                1 + seed(key) % 28,
                8 + seed(&format!("{key}h")) % 10
            ));
        }
        if format == "date" || field.ends_with("_date") || field == "date" {
            return Value::String(format!("2026-09-{:02}", 1 + seed(key) % 28));
        }
        if format == "email" || field.contains("email") {
            return Value::String(PEOPLE[0].1.to_owned());
        }
        if format == "duration" {
            return Value::String("P7D".to_owned());
        }
        if format == "time" {
            return Value::String("09:00:00Z".to_owned());
        }
        if format == "ipv4" {
            return Value::String("127.0.0.1".to_owned());
        }
        if format == "hostname" {
            return Value::String("example.com".to_owned());
        }
        let longest = schema["maxLength"].as_u64().unwrap_or(u64::MAX);
        if ["avatar", "image", "logo", "picture", "photo", "icon"]
            .iter()
            .any(|w| field.contains(w))
        {
            let picture = data_uri(PICTURE);
            if (picture.len() as u64) <= longest {
                return Value::String(picture);
            }
        }
        if format == "uri" || format == "url" || field.ends_with("url") || field == "website" {
            return Value::String(format!("https://example.com/{}", field.replace('_', "-")));
        }
        let word = match field {
            "slug" | "handle" | "username" | "login" => "demo",
            _ if field.contains("currency") => "usd",
            _ if field.contains("country") => "US",
            _ if field.contains("locale") || field == "language" => "en",
            _ if field.contains("timezone") => "Europe/Paris",
            "status" | "state" => "active",
            "first_name" | "firstname" => "Ada",
            "last_name" | "lastname" => "Lovelace",
            "title" => "Pro plan",
            "description" | "summary" | "bio" => "Everything a growing team needs.",
            "phone" => "+1 555 0100",
            "color" | "colour" => "#6366f1",
            _ if field.contains("name") => "Demo",
            _ => "",
        };
        if !word.is_empty() {
            return Value::String(word.to_owned());
        }
        let shown = field.replace(['_', '-'], " ");
        let mut letters = shown.chars();
        Value::String(match letters.next() {
            Some(first) => first.to_uppercase().chain(letters).collect(),
            None => "Text".to_owned(),
        })
    }
}

/// A string made to fit its schema: its pattern, matched by a string tried
/// in turn when it does not, and its lengths.
fn fitted(schema: &Value, made: Value) -> Value {
    let Value::String(mut text) = made else {
        return made;
    };
    if let Some(pattern) = schema["pattern"].as_str()
        && let Ok(pattern) = regex::Regex::new(&without_lookaround(pattern))
        && !pattern.is_match(&text)
        && let Some(tried) = PATTERNED.iter().find(|t| pattern.is_match(t))
    {
        (*tried).clone_into(&mut text);
    }
    if let Some(longest) = schema["maxLength"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
    {
        text = text.chars().take(longest).collect();
    }
    if let Some(shortest) = schema["minLength"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
    {
        while text.chars().count() < shortest {
            text.push('x');
        }
    }
    Value::String(text)
}

/// A pattern without what Rust's regular expressions do not read, its
/// look-arounds: `(?!^[-+.]*$)` around a decimal's.
fn without_lookaround(pattern: &str) -> String {
    let mut kept = String::new();
    let mut rest = pattern;
    while let Some(start) = ["(?=", "(?!", "(?<=", "(?<!"]
        .iter()
        .filter_map(|open| rest.find(open))
        .min()
    {
        kept.push_str(&rest[..start]);
        let mut depth = 0usize;
        let mut end = rest.len();
        let mut escaped = false;
        for (i, c) in rest[start..].char_indices() {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = &rest[end..];
    }
    kept.push_str(rest);
    kept
}

/// A number held within its schema's bounds.
fn within_bounds(schema: &Value, made: Value) -> Value {
    let Some(number) = made.as_f64() else {
        return made;
    };
    let low = schema["minimum"]
        .as_f64()
        .or_else(|| schema["exclusiveMinimum"].as_f64().map(|m| m + 1.0));
    let high = schema["maximum"]
        .as_f64()
        .or_else(|| schema["exclusiveMaximum"].as_f64().map(|m| m - 1.0));
    let held = match (low, high) {
        (Some(low), Some(high)) if low <= high => number.clamp(low, high),
        (Some(low), _) => number.max(low),
        (_, Some(high)) => number.min(high),
        _ => return made,
    };
    if made.is_i64() || made.is_u64() {
        json!(held.round() as i64)
    } else {
        json!(held)
    }
}

/// A `data:` address of an SVG picture, each character that is not one an
/// address may hold as it is escaped: a URI as its schema checks it.
fn data_uri(svg: &str) -> String {
    let mut uri = "data:image/svg+xml,".to_owned();
    for byte in svg.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// An item of a list made its own: a person's name and email when it has
/// them, else a name numbered after the first's.
fn distinct(item: Value, i: usize) -> Value {
    let Value::Object(mut fields) = item else {
        return item;
    };
    // A date a spec's example gives every item alike is a day apart on
    // each: a page keys its rows by them.
    for (name, value) in &mut fields {
        let dated = name.ends_with("_at")
            || ["timestamp", "date", "day", "period"].contains(&name.as_str());
        if let Value::String(date) = value
            && dated
            && i > 0
            && let Some(later) = days_later(date, i)
        {
            *date = later;
        }
    }
    let (name, email) = PEOPLE[i % PEOPLE.len()];
    if fields.get("email").is_some_and(Value::is_string) {
        fields.insert("email".to_owned(), Value::String(email.to_owned()));
        if fields.get("name").is_some_and(Value::is_string) {
            fields.insert("name".to_owned(), Value::String(name.to_owned()));
        }
    } else if let Some(Value::String(own)) = fields.get("name")
        && i > 0
    {
        let numbered = format!("{own} {}", i + 1);
        fields.insert("name".to_owned(), Value::String(numbered));
    }
    Value::Object(fields)
}

/// `date` (`2026-01-01…`) `days` later, within its month: a day of the
/// month past 28 wraps around.
fn days_later(date: &str, days: usize) -> Option<String> {
    let day: usize = date.get(8..10)?.parse().ok()?;
    if date.get(4..5) != Some("-") || date.get(7..8) != Some("-") {
        return None;
    }
    let later = (day - 1 + days) % 28 + 1;
    Some(format!("{}{later:02}{}", &date[..8], &date[10..]))
}

/// The entity a schema stands for, as a field names it: `organization` for
/// `#/components/schemas/OrganizationWithRole` or `OrganizationRead`.
fn entity(reference: &str) -> String {
    let name = reference.rsplit('/').next().unwrap_or(reference);
    let name = name.split("With").next().unwrap_or(name);
    let mut name = name.to_owned();
    for suffix in [
        "Read", "Schema", "Public", "Response", "Output", "Out", "Details", "Detail", "Full",
        "Base", "Item", "Dto", "Model", "Data",
    ] {
        if name.len() > suffix.len() && name.ends_with(suffix) {
            name.truncate(name.len() - suffix.len());
            break;
        }
    }
    name.to_lowercase()
}

/// A thing's place in the list it is in, from where it is made: the last
/// `[n]` of the place; 0 for one alone.
fn place(key: &str) -> usize {
    key.rsplit_once('[')
        .and_then(|(_, rest)| rest.split(']').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// A number from a place, the same each time.
fn seed(key: &str) -> u32 {
    // FNV-1a: small, and the same everywhere.
    let mut hash: u32 = 0x811c_9dc5;
    for byte in key.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// An id that looks like one, from a place.
fn uuid(key: &str) -> String {
    let a = seed(key);
    let b = seed(&format!("{key}#"));
    let c = seed(&format!("{key}##"));
    format!(
        "{a:08x}-{:04x}-4{:03x}-a{:03x}-{:08x}{:04x}",
        b >> 16,
        b & 0xfff,
        c >> 20,
        c,
        a & 0xffff
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Spec {
        Spec::read(
            r##"{"paths": {"/me": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Session"}}}}}}}},
              "components": {"schemas": {
                "Session": {"type": "object", "properties": {
                    "user": {"$ref": "#/components/schemas/User"},
                    "role": {"enum": ["owner", "admin", "member"]},
                    "is_admin": {"type": "boolean"},
                    "permissions": {"type": "array", "items": {"$ref": "#/components/schemas/Permission"}},
                    "compass_enabled": {"type": "boolean"},
                    "canceled_at": {"anyOf": [{"type": "string", "format": "date-time"}, {"type": "null"}]},
                    "created_at": {"anyOf": [{"type": "string", "format": "date-time"}, {"type": "null"}]},
                    "customers": {"$ref": "#/components/schemas/Page"}}},
                "User": {"type": "object", "properties": {"id": {"type": "string"}, "email": {"type": "string"}, "avatar_url": {"type": "string"}, "manager": {"$ref": "#/components/schemas/User"}}},
                "Permission": {"type": "string", "enum": ["products:read", "products:write"]},
                "Page": {"type": "object", "properties": {"items": {"type": "array", "items": {"$ref": "#/components/schemas/Customer"}}, "total_count": {"type": "integer"}}},
                "Customer": {"type": "object", "properties": {"id": {"type": "string", "format": "uuid"}, "name": {"type": "string", "example": "John Doe"}, "email": {"type": "string", "example": "john@example.com"}}}}}}"##,
        )
        .unwrap()
    }

    fn session(tone: Tone) -> Value {
        let spec = spec();
        let schema = spec.schema(0, "200").unwrap().clone();
        body(&spec, &schema, "GET /me 200", tone)
    }

    #[test]
    fn a_body_looks_like_what_its_fields_name() {
        let made = session(Tone::default());
        assert_eq!(made, session(Tone::default()));
        assert_eq!(made["role"], "owner");
        assert_eq!(made["is_admin"], false);
        assert_eq!(made["compass_enabled"], false);
        assert_eq!(
            made["permissions"],
            json!(["products:read", "products:write"])
        );
        assert_eq!(made["canceled_at"], Value::Null);
        assert!(made["created_at"].as_str().unwrap().starts_with("2026-09-"));
        assert_eq!(made["user"]["email"], "ada@example.com");
        assert!(
            made["user"]["avatar_url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/svg+xml")
        );
        assert_eq!(made["user"]["id"].as_str().unwrap().len(), 36);
        // A schema that holds itself stops.
        assert!(made["user"]["manager"]["manager"]["manager"].is_null());
        // Each customer its own.
        let items = made["customers"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_ne!(items[0]["id"], items[1]["id"]);
        assert_eq!(items[0]["name"], "Ada Lovelace");
        assert_eq!(items[1]["email"], "grace@example.com");
        assert_eq!(made["customers"]["total_count"], 3);
    }

    #[test]
    fn the_same_thing_has_the_same_id_in_every_answer() {
        let spec = Spec::read(
            r##"{"paths": {
                "/orgs": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"type": "array", "items": {"$ref": "#/components/schemas/Organization"}}}}}}}},
                "/me": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"type": "object", "properties": {"organizations": {"type": "array", "items": {"$ref": "#/components/schemas/OrganizationWithRole"}}}}}}}}}},
                "/orders/1": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/OrderRead"}}}}}}}},
              "components": {"schemas": {
                "Organization": {"type": "object", "properties": {"id": {"type": "string"}}},
                "OrganizationWithRole": {"type": "object", "properties": {"id": {"type": "string"}, "role": {"type": "string"}}},
                "OrderRead": {"type": "object", "properties": {"id": {"type": "string"}, "organization_id": {"type": "string"}}}}}}"##,
        )
        .unwrap();
        let answer = |path: &str| {
            let index = spec.find("GET", path).unwrap();
            body(
                &spec,
                spec.schema(index, "200").unwrap(),
                path,
                Tone::default(),
            )
        };
        let (orgs, me, order) = (answer("/orgs"), answer("/me"), answer("/orders/1"));
        assert_eq!(orgs[0]["id"], me["organizations"][0]["id"]);
        assert_eq!(orgs[1]["id"], me["organizations"][1]["id"]);
        assert_ne!(orgs[0]["id"], orgs[1]["id"]);
        assert_eq!(order["organization_id"], orgs[0]["id"]);
        assert_ne!(order["id"], orgs[0]["id"]);
    }

    #[test]
    fn dates_alike_are_a_day_apart() {
        assert_eq!(
            days_later("2026-01-01T00:00:00Z", 2).as_deref(),
            Some("2026-01-03T00:00:00Z")
        );
        assert_eq!(days_later("2026-01-27", 3).as_deref(), Some("2026-01-02"));
        assert_eq!(days_later("Monday", 1), None);
        let item = json!({"timestamp": "2026-01-01T00:00:00Z", "name": "Row"});
        assert_eq!(distinct(item, 1)["timestamp"], "2026-01-02T00:00:00Z");
    }

    /// The body made for a lone schema, with these schemas beside it.
    fn made_for(schema: &str, schemas: &str) -> Value {
        let spec = Spec::read(&format!(
            r#"{{"paths": {{"/x": {{"get": {{"responses": {{"200": {{"content": {{"application/json": {{"schema": {schema}}}}}}}}}}}}}}},
               "components": {{"schemas": {schemas}}}}}"#
        ))
        .unwrap();
        body(
            &spec,
            spec.schema(0, "200").unwrap(),
            "GET /x 200",
            Tone::default(),
        )
    }

    #[test]
    fn a_body_holds_to_its_schema() {
        // A tuple: as its example has it, else each place its own.
        let made = made_for(
            r##"{"type": "object", "properties": {
                "tax_id": {"anyOf": [{"type": "array", "prefixItems": [{"type": "string"}, {"$ref": "#/components/schemas/Format"}], "minItems": 2, "maxItems": 2, "examples": [["911144442", "us_ein"]]}, {"type": "null"}]},
                "pair": {"type": "array", "prefixItems": [{"type": "integer"}, {"$ref": "#/components/schemas/Format"}]},
                "few": {"type": "array", "items": {"type": "string"}, "maxItems": 1},
                "many": {"type": "array", "items": {"type": "integer"}, "minItems": 5},
                "share": {"type": "string", "pattern": "^(?!^[-+.]*$)[+-]?0*\\d*\\.?\\d*$"},
                "delay": {"type": "string", "format": "duration"},
                "logo_url": {"type": "string", "format": "uri"},
                "percent": {"type": "integer", "minimum": 0, "maximum": 10},
                "price": {"oneOf": [{"$ref": "#/components/schemas/Fixed"}, {"$ref": "#/components/schemas/Free"}],
                          "discriminator": {"propertyName": "kind", "mapping": {"zero": "#/components/schemas/Free", "fixed": "#/components/schemas/Fixed"}}},
                "filter": {"$ref": "#/components/schemas/Filter"}}}"##,
            r##"{"Format": {"type": "string", "enum": ["eu_vat", "us_ein"]},
                "Fixed": {"type": "object", "properties": {"kind": {"type": "string"}, "amount": {"type": "integer"}}},
                "Free": {"type": "object", "properties": {"kind": {"type": "string"}}},
                "Filter": {"type": "object", "properties": {"clauses": {"type": "array", "items": {"anyOf": [{"$ref": "#/components/schemas/Filter"}, {"$ref": "#/components/schemas/Clause"}]}}}},
                "Clause": {"type": "object", "properties": {"value": {"type": "string"}}}}"##,
        );
        assert_eq!(made["tax_id"], json!(["911144442", "us_ein"]));
        assert_eq!(made["pair"][1], "eu_vat");
        assert_eq!(made["pair"].as_array().unwrap().len(), 2);
        assert_eq!(made["few"].as_array().unwrap().len(), 1);
        assert_eq!(made["many"].as_array().unwrap().len(), 5);
        assert_eq!(made["share"], "12.50");
        assert_eq!(made["delay"], "P7D");
        let logo = made["logo_url"].as_str().unwrap();
        assert!(logo.starts_with("data:image/svg+xml,%3Csvg"));
        assert!(!logo.contains([' ', '<', '\'', '#']));
        assert!(made["percent"].as_i64().unwrap() <= 10);
        // The spec's first kind, its discriminator's property saying so.
        assert_eq!(made["price"]["kind"], "fixed");
        assert!(made["price"]["amount"].is_number());
        // A filter of filters ends with a clause, not with nothing.
        assert_eq!(made["filter"]["clauses"][0]["value"], "Value");
    }

    #[test]
    fn what_nests_too_deep_is_empty_not_null() {
        let made = made_for(
            r##"{"$ref": "#/components/schemas/Level"}"##,
            r##"{"Level": {"type": "object", "properties": {"name": {"type": "string"}, "below": {"type": "array", "items": {"$ref": "#/components/schemas/Level2"}}}},
                "Level2": {"type": "object", "properties": {"name": {"type": "string"}, "below": {"type": "array", "items": {"$ref": "#/components/schemas/Level3"}}}},
                "Level3": {"type": "object", "properties": {"name": {"type": "string"}, "below": {"type": "array", "items": {"$ref": "#/components/schemas/Level4"}}}},
                "Level4": {"type": "object", "properties": {"name": {"type": "string"}, "below": {"type": "array", "items": {"$ref": "#/components/schemas/Level5"}}}},
                "Level5": {"type": "object", "properties": {"name": {"type": "string"}, "below": {"type": "array", "items": {"$ref": "#/components/schemas/Level6"}}}},
                "Level6": {"type": "object", "properties": {"name": {"type": "string"}, "below": {"type": "array", "items": {"$ref": "#/components/schemas/Level7"}}}},
                "Level7": {"type": "object", "properties": {"name": {"type": "string"}}}}"##,
        );
        let mut level = &made;
        let mut depth = 0;
        while let Some(first) = level["below"].get(0) {
            assert!(first["name"].is_string());
            level = first;
            depth += 1;
        }
        assert!(depth >= 4);
        assert!(level["below"].is_null() || level["below"] == json!([]));
    }

    #[test]
    fn a_tone_bends_the_body_to_a_state() {
        let empty = session(Tone {
            empty: true,
            ..Tone::default()
        });
        assert_eq!(empty["customers"]["items"], json!([]));
        assert_eq!(empty["customers"]["total_count"], 0);
        let no_rights = session(Tone {
            no_rights: true,
            ..Tone::default()
        });
        assert_eq!(no_rights["permissions"], json!([]));
        assert_eq!(no_rights["role"], "member");
        let flags = session(Tone {
            flags_on: true,
            ..Tone::default()
        });
        assert_eq!(flags["compass_enabled"], true);
        assert_eq!(flags["is_admin"], false);
    }
}

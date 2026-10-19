//! What the simulated back end holds, built up as the page goes: the items
//! it showed, as their lists first made them, and those the page created,
//! changed and deleted, each known by its id wherever it shows. An item
//! renamed reads renamed in its list, on its page and within every answer
//! that holds it (an order's product, a product's perks); one deleted
//! leaves every list it was in.
//!
//! What an operation does is read from the spec's paths as REST has them:
//! `POST /items/` adds to what `GET /items/` lists and `GET /items/{id}`
//! reads, `PATCH` and `PUT` change one, `DELETE` removes it, `PATCH /me`
//! changes what `GET /me` reads. An action on an item answered with the
//! item, `POST /products/{id}/perks`, changes it with what it was given,
//! and with what its name says when the item's fields name it
//! (`/archive`, `/cancel`). A creation answered with the items of a list
//! elsewhere, `POST /teams/{id}/members`, adds to that list, the
//! team's id its own. Ids given where an answer holds objects
//! (`perks: [ids]`, `product_id`) become those objects.
//!
//! What a real server computes beyond what it is told (an order made when
//! a checkout succeeds, a total summed) is not guessed.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::made::{COUNTS, LISTS, entity, one_of, uuid};
use crate::spec::Spec;

/// What an operation does to what the back end holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Role {
    /// Lists a collection, `GET /items/`: of items one of which another
    /// operation reads, or that have ids.
    List {
        /// The parameter that names its items, `id` for `/items/{id}`.
        param: String,
    },
    /// Adds to a collection: `POST /items/`.
    Create {
        /// The parameter that names its items.
        param: String,
        /// Whether the collection is another path's, the list it is
        /// answered with an item of.
        elsewhere: bool,
    },
    /// Reads an item, `GET /items/{id}`, or a thing alone, `GET /me`, that
    /// is changed where it is read.
    Read {
        /// The parameter that names the item; none for a thing alone.
        param: Option<String>,
    },
    /// Changes one: `PATCH /items/{id}`, `PUT`, `PATCH /me`.
    Update {
        /// The parameter that names the item; none for a thing alone.
        param: Option<String>,
    },
    /// Removes an item: `DELETE /items/{id}`.
    Delete {
        /// The parameter that names it.
        param: String,
    },
    /// Acts on an item and answers with it: `POST /items/{id}/archive`.
    Act {
        /// The parameter that names the item.
        param: String,
        /// What the path's last part says is done: `archive`.
        verb: String,
    },
    /// Nothing the others follow.
    Other,
}

/// Each operation's role, and the operation that lists its collection.
pub(crate) fn roles(spec: &Spec) -> Vec<(Role, Option<usize>)> {
    let operations = &spec.operations;
    let under = |parts: &[Option<String>], theirs: &[Option<String>]| {
        theirs.len() == parts.len() + 1
            && theirs.last() == Some(&None)
            && theirs[..parts.len()] == *parts
    };
    // The parameter of the operations on one item under `parts`.
    let item = |parts: &[Option<String>]| {
        operations
            .iter()
            .find(|o| under(parts, o.parts()))
            .map(|o| param_name(&o.path))
    };
    let read_at = |parts: &[Option<String>]| {
        operations
            .iter()
            .position(|o| o.method == "GET" && under(parts, o.parts()))
    };
    let lists = |index: usize| {
        let parts = operations[index].parts();
        operations[index].method == "GET"
            && matches!(parts.last(), Some(Some(_)))
            && (item(parts).is_some() || spec.listed(index).is_some_and(|i| spec.names_items(i)))
    };
    let list_at = |parts: &[Option<String>]| {
        operations
            .iter()
            .position(|o| o.method == "GET" && o.parts() == parts)
            .filter(|index| lists(*index))
    };
    let answered = |index: usize| spec.success_schema(index).and_then(Spec::reference);
    let named = |parts: &[Option<String>]| item(parts).unwrap_or_else(|| "id".to_owned());
    operations
        .iter()
        .enumerate()
        .map(|(index, operation)| {
            let parts = operation.parts();
            let method = operation.method.as_str();
            let Some((last, above)) = parts.split_last() else {
                return (Role::Other, None);
            };
            let Some(verb) = last else {
                let param = param_name(&operation.path);
                let role = match method {
                    "GET" => Role::Read { param: Some(param) },
                    "PATCH" | "PUT" => Role::Update { param: Some(param) },
                    "DELETE" => Role::Delete { param },
                    _ => Role::Other,
                };
                return (role, list_at(above));
            };
            // An action on an item, answered with the item itself, as it
            // is read alone or listed.
            if let Some((None, collection)) = above.split_last()
                && !matches!(method, "GET" | "DELETE")
                && let Some(made) = answered(index)
                && let Some(param) = item(collection)
                && (read_at(collection).and_then(answered) == Some(made)
                    || list_at(collection)
                        .and_then(|l| spec.listed(l))
                        .and_then(Spec::reference)
                        == Some(made))
            {
                let role = Role::Act {
                    param,
                    verb: verb.clone(),
                };
                return (role, list_at(collection));
            }
            let list = list_at(parts);
            let changed_there = operations
                .iter()
                .any(|o| o.parts() == parts && matches!(o.method.as_str(), "PATCH" | "PUT"));
            match method {
                "GET" if list.is_some() => (
                    Role::List {
                        param: named(parts),
                    },
                    list,
                ),
                "POST" if list.is_some() => (
                    Role::Create {
                        param: named(parts),
                        elsewhere: false,
                    },
                    list,
                ),
                // A creation answered with an item of what its path names,
                // listed elsewhere: `/teams/{id}/members`, not
                // `/invites/{id}/resend`.
                "POST" if answered(index).is_some_and(|made| names(verb, made)) => {
                    let elsewhere = answered(index).and_then(|made| {
                        operations
                            .iter()
                            .enumerate()
                            .filter(|(l, o)| {
                                o.parts().iter().all(Option::is_some)
                                    && lists(*l)
                                    && spec.listed(*l).and_then(Spec::reference) == Some(made)
                            })
                            .max_by_key(|(l, o)| {
                                let shared = o
                                    .parts()
                                    .iter()
                                    .zip(parts)
                                    .take_while(|(a, b)| a == b)
                                    .count();
                                (shared, usize::MAX - o.parts().len(), usize::MAX - l)
                            })
                            .map(|(l, _)| l)
                    });
                    match elsewhere {
                        Some(list) => (
                            Role::Create {
                                param: named(operations[list].parts()),
                                elsewhere: true,
                            },
                            Some(list),
                        ),
                        None => (Role::Other, None),
                    }
                }
                "GET" if changed_there => (Role::Read { param: None }, None),
                "PATCH" | "PUT" => (Role::Update { param: None }, None),
                _ => (Role::Other, None),
            }
        })
        .collect()
}

/// Whether a path's last part names the entity of a schema: `members`
/// that of `#/components/schemas/MemberRead`.
fn names(part: &str, reference: &str) -> bool {
    let noun = one_of(part).replace(['-', '_'], "").to_lowercase();
    let kind = entity(reference);
    !noun.is_empty() && (kind.contains(&noun) || noun.contains(&kind))
}

/// A parameter's name, from the last part of a path: `id` for
/// `/items/{id}`.
fn param_name(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim_matches(['{', '}'])
        .to_owned()
}

/// The fields a path's parameters fill in what is created under it, as
/// the parts before them name them: `team_id` for the `{id}` of
/// `/teams/{id}/members`.
pub(crate) fn links(template: &[Option<String>], parts: &[String]) -> Vec<(String, String)> {
    template
        .windows(2)
        .zip(parts.iter().skip(1))
        .filter_map(|(pair, value)| match pair {
            [Some(owner), None] => Some((
                format!("{}_id", one_of(&owner.replace('-', "_"))),
                value.clone(),
            )),
            _ => None,
        })
        .collect()
}

/// What the back end holds of a collection beyond what the spec makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    /// The collection's path, `/v1/products`, or a thing alone's,
    /// `/v1/users/me`.
    pub path: String,
    /// How many items a page created there, and kept.
    pub created: usize,
    /// How many items it changed through it.
    pub changed: usize,
    /// How many it deleted.
    pub deleted: usize,
}

/// An item asked for that was deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Gone;

/// What the back end holds, each item by its key (see [`item_key`]).
#[derive(Debug, Default)]
pub(crate) struct Records {
    /// What was done through each collection, by its path, `v1/products`.
    collections: BTreeMap<String, Collection>,
    /// The items created, as their creation answered them.
    created: BTreeMap<String, Value>,
    /// The keys of the items created of each kind, the schema their lists
    /// list (`#/components/schemas/Member`), in turn: a member created
    /// from a team is among every list of members.
    kinds: BTreeMap<String, Vec<String>>,
    /// What was changed of an item, as the requests gave it; of a thing
    /// alone, by its path, `/v1/users/me`.
    changed: BTreeMap<String, Value>,
    /// The items deleted.
    deleted: BTreeSet<String>,
    /// The items as the lists made them, for an item to read everywhere
    /// as its list showed it.
    seen: BTreeMap<String, Value>,
    /// The collection each item was seen or created in, by its key.
    homes: BTreeMap<String, String>,
}

#[derive(Debug, Default)]
struct Collection {
    /// The keys of the items created there and not deleted, in turn.
    created: Vec<String>,
    /// How many were ever created there, for each to have an id of its own.
    made: usize,
    /// The ids of the items changed through it.
    changed: BTreeSet<String>,
    /// The ids of those deleted.
    deleted: BTreeSet<String>,
}

impl Collection {
    fn is_empty(&self) -> bool {
        self.created.is_empty() && self.changed.is_empty() && self.deleted.is_empty()
    }
}

impl Records {
    /// What it holds of what the page did, by collection.
    pub(crate) fn held(&self) -> Vec<Held> {
        self.collections
            .iter()
            .filter(|(_, c)| !c.is_empty())
            .map(|(path, c)| Held {
                path: format!("/{path}"),
                created: c.created.len(),
                changed: c.changed.len(),
                deleted: c.deleted.len(),
            })
            .collect()
    }

    /// Forgets all it holds: every answer as the spec makes it again.
    pub(crate) fn forget(&mut self) {
        *self = Self::default();
    }

    /// Forgets how the lists made their items, made otherwise from now
    /// on: in another state, flags on, rights withheld.
    pub(crate) fn unsee(&mut self) {
        self.seen.clear();
    }

    /// Lays what it holds over a list's answer, the list of `key`: those
    /// deleted out, those created of its `kind` first, the latest first,
    /// those of its path's `links` only (a team's members), each shaped
    /// as `template` makes an item of the list, what was changed of each;
    /// then the list narrowed to what its query asks, and counted. The
    /// items made are seen, for each to read as they do. Whether what the
    /// page did shows there.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn list(
        &mut self,
        key: &str,
        kind: &str,
        param: &str,
        body: &mut Value,
        template: impl FnOnce() -> Option<Value>,
        query: &[(String, String)],
        links: &[(String, String)],
    ) -> bool {
        let Some(items) = list_in(body) else {
            return false;
        };
        for item in items.iter() {
            let id = id_of(item, param);
            if !id.is_empty() {
                let k = item_key(key, &id);
                self.homes.insert(k.clone(), key.to_owned());
                self.seen.insert(k, item.clone());
            }
        }
        let before = items.len();
        items.retain(|item| !self.deleted.contains(&item_key(key, &id_of(item, param))));
        let mut held = items.len() != before;
        let linked = |record: &&Value| {
            links
                .iter()
                .all(|(field, value)| record.get(field).is_none_or(|v| shown(v) == *value))
        };
        let created: Vec<&Value> = self
            .kinds
            .get(kind)
            .into_iter()
            .flatten()
            .rev()
            .filter_map(|k| self.created.get(k))
            .filter(linked)
            .collect();
        if !created.is_empty() {
            let template = template();
            let created: Vec<Value> = created
                .into_iter()
                .map(|record| {
                    let mut item = template.clone().unwrap_or_else(|| record.clone());
                    laid(&mut item, record, key);
                    item
                })
                .collect();
            items.splice(0..0, created);
            held = true;
        }
        for item in items.iter_mut() {
            let k = item_key(key, &id_of(item, param));
            if let Some(change) = self.changed.get(&k) {
                overlay(item, change, &k);
                held = true;
            }
        }
        let narrowed = narrow(items, query);
        let count = items.len();
        if held || narrowed {
            recount(body, count, 0);
        }
        held
    }

    /// Adds an item of `kind` to `key`: what the creation answers when it
    /// names the item, else an item of the list as `template` makes it,
    /// with the fields `links` fills, what the request gave and ids of its
    /// own. The answer as the creation's schema has it, and the item.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create(
        &mut self,
        key: &str,
        kind: &str,
        param: &str,
        given: Option<&Value>,
        answer: Option<Value>,
        template: impl FnOnce() -> Option<Value>,
        links: &[(String, String)],
    ) -> (Option<Value>, Value) {
        let named = answer
            .as_ref()
            .filter(|a| a.get(id_field(a, param)).is_some())
            .cloned();
        let mut record = named
            .or_else(template)
            .or_else(|| answer.clone().filter(Value::is_object))
            .unwrap_or_else(|| json!({}));
        let collection = self.collections.entry(key.to_owned()).or_default();
        let n = collection.made;
        collection.made += 1;
        let place = format!("{key}+{n}");
        let linked: Map<String, Value> = links
            .iter()
            .map(|(field, value)| (field.clone(), Value::String(value.clone())))
            .collect();
        overlay(&mut record, &Value::Object(linked), &place);
        if let Some(given) = given {
            overlay(&mut record, given, &place);
        }
        let field = id_field(&record, param).to_owned();
        for field in ["id", field.as_str()] {
            let given_one = given.and_then(|g| g.get(field)).is_some();
            if let Some(slot) = record.get_mut(field).filter(|_| !given_one) {
                *slot = fresh(slot, key, n);
            }
        }
        let k = item_key(key, &id_of(&record, param));
        collection.created.push(k.clone());
        self.kinds
            .entry(kind.to_owned())
            .or_default()
            .push(k.clone());
        self.homes.insert(k.clone(), key.to_owned());
        self.created.insert(k, record.clone());
        let answer = match answer {
            Some(mut answer) if answer.is_object() => {
                overlay(&mut answer, &record, &place);
                Some(answer)
            }
            other => other,
        };
        (answer, record)
    }

    /// The key of the item of id `id`, wherever it was seen or created:
    /// the id, else its id within its collection.
    fn key_of(&self, id: &str) -> Option<String> {
        let known = |k: &String| {
            self.created.contains_key(k)
                || self.seen.contains_key(k)
                || self.changed.contains_key(k)
        };
        let own = id.to_owned();
        if known(&own) || self.deleted.contains(&own) {
            return Some(own);
        }
        let within = format!("#{id}");
        self.homes
            .keys()
            .chain(self.changed.keys())
            .find(|k| k.ends_with(&within))
            .cloned()
    }

    /// The item of id `id` as it reads now: as it was created or seen,
    /// with what was changed of it; none when it was never seen, or was
    /// deleted.
    pub(crate) fn item(&self, id: &str) -> Option<Value> {
        let k = self.key_of(id)?;
        if self.deleted.contains(&k) {
            return None;
        }
        let change = self.changed.get(&k);
        let mut item = self
            .created
            .get(&k)
            .or_else(|| self.seen.get(&k))
            .or(change)
            .cloned()?;
        if let Some(change) = change {
            overlay(&mut item, change, &k);
        }
        Some(item)
    }

    /// Changes the item of id `id` with `fields`, wherever it shows.
    ///
    /// # Errors
    ///
    /// [`Gone`] when it was deleted.
    pub(crate) fn change_item(&mut self, id: &str, fields: &Value) -> Result<(), Gone> {
        let k = self.key_of(id).unwrap_or_else(|| id.to_owned());
        if self.deleted.contains(&k) {
            return Err(Gone);
        }
        merge(
            self.changed.entry(k.clone()).or_insert_with(|| json!({})),
            fields,
        );
        let home = self.homes.get(&k).cloned().unwrap_or_default();
        self.collections
            .entry(home)
            .or_default()
            .changed
            .insert(id.to_owned());
        Ok(())
    }

    /// Deletes the item of id `id`, wherever it shows.
    ///
    /// # Errors
    ///
    /// [`Gone`] when it was deleted already.
    pub(crate) fn delete_item(&mut self, id: &str) -> Result<(), Gone> {
        let k = self.key_of(id).unwrap_or_else(|| id.to_owned());
        let home = self.homes.get(&k).cloned().unwrap_or_default();
        self.delete(&home, id)
    }

    /// Item `id` of `key` as it reads now: `made`, as the spec makes it,
    /// with the item as it was created or listed (`seeds`) laid over it,
    /// else the id asked as its own, and what was changed of it. A thing
    /// alone has no parameter. Whether what the page did shows there.
    ///
    /// # Errors
    ///
    /// [`Gone`] when the item was deleted.
    pub(crate) fn read(
        &mut self,
        key: &str,
        param: Option<&str>,
        id: &str,
        seeds: impl FnOnce() -> Vec<Value>,
        made: Option<Value>,
    ) -> Result<(Option<Value>, bool), Gone> {
        let Some(param) = param else {
            let Some(mut answer) = made else {
                return Ok((None, false));
            };
            let change = self.changed.get(&format!("/{key}"));
            if let Some(change) = change {
                overlay(&mut answer, change, key);
            }
            return Ok((Some(answer), change.is_some()));
        };
        let k = item_key(key, id);
        if self.deleted.contains(&k) {
            return Err(Gone);
        }
        let Some(mut answer) = made else {
            return Ok((None, false));
        };
        let mut held = false;
        if let Some(record) = self.created.get(&k) {
            laid(&mut answer, record, &k);
            held = true;
        } else {
            if !self.seen.contains_key(&k) {
                for seed in seeds() {
                    let seed_id = id_of(&seed, param);
                    if !seed_id.is_empty() {
                        let seed_key = item_key(key, &seed_id);
                        self.homes.insert(seed_key.clone(), key.to_owned());
                        self.seen.insert(seed_key, seed);
                    }
                }
            }
            match self.seen.get(&k) {
                Some(seed) if !other_kind(&answer, seed) => overlay(&mut answer, seed, &k),
                Some(_) => {}
                None => own_id(&mut answer, param, id),
            }
        }
        if let Some(change) = self.changed.get(&k) {
            overlay(&mut answer, change, &k);
            held = true;
        }
        Ok((Some(answer), held))
    }

    /// Changes item `id` of `key` with what the request gave and what was
    /// `done` to it, then reads it as [`Records::read`] does.
    ///
    /// # Errors
    ///
    /// [`Gone`] when the item was deleted.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn update(
        &mut self,
        key: &str,
        param: Option<&str>,
        id: &str,
        given: Option<&Value>,
        done: Option<Value>,
        seeds: impl FnOnce() -> Vec<Value>,
        made: Option<Value>,
    ) -> Result<(Option<Value>, bool), Gone> {
        let k = match param {
            Some(_) => item_key(key, id),
            None => format!("/{key}"),
        };
        if self.deleted.contains(&k) {
            return Err(Gone);
        }
        let collection = self.collections.entry(key.to_owned()).or_default();
        let changes = given.into_iter().chain(done.as_ref());
        for change in
            changes.filter(|c| !c.is_null() && c.as_object().is_none_or(|o| !o.is_empty()))
        {
            merge(
                self.changed.entry(k.clone()).or_insert_with(|| json!({})),
                change,
            );
            collection.changed.insert(id.to_owned());
        }
        self.read(key, param, id, seeds, made)
    }

    /// Deletes item `id` of `key`.
    ///
    /// # Errors
    ///
    /// [`Gone`] when it was deleted already.
    pub(crate) fn delete(&mut self, key: &str, id: &str) -> Result<(), Gone> {
        let k = item_key(key, id);
        if !self.deleted.insert(k.clone()) {
            return Err(Gone);
        }
        self.created.remove(&k);
        self.changed.remove(&k);
        for keys in self.kinds.values_mut() {
            keys.retain(|c| *c != k);
        }
        let collection = self.collections.entry(key.to_owned()).or_default();
        collection.created.retain(|c| *c != k);
        collection.changed.remove(id);
        collection.deleted.insert(id.to_owned());
        Ok(())
    }

    /// Lays what it holds over every item an answer holds, at any depth:
    /// each reads as its list showed it, as it was created, with what was
    /// changed of it; one deleted leaves the lists it is in. Whether what
    /// the page did shows there.
    pub(crate) fn lay(&self, answer: &mut Value) -> bool {
        let mut held = false;
        self.walk(answer, &mut held, 0);
        held
    }

    fn walk(&self, value: &mut Value, held: &mut bool, depth: usize) {
        if depth > 32 {
            return;
        }
        if let Some(key) = own_key(value).map(str::to_owned) {
            if let Some(seen) = self.seen.get(&key).filter(|seen| !other_kind(value, seen)) {
                overlay(value, seen, &key);
            }
            if let Some(record) = self.created.get(&key) {
                laid(value, record, &key);
                *held = true;
            }
            if let Some(change) = self.changed.get(&key) {
                overlay(value, change, &key);
                *held = true;
            }
        }
        match value {
            Value::Array(items) => {
                let before = items.len();
                items.retain(|item| own_key(item).is_none_or(|key| !self.deleted.contains(key)));
                *held |= items.len() != before;
                for item in items {
                    self.walk(item, held, depth + 1);
                }
            }
            Value::Object(fields) => {
                for field in fields.values_mut() {
                    self.walk(field, held, depth + 1);
                }
            }
            _ => {}
        }
    }
}

/// What an action a path's last part names does to an item, as the item's
/// fields say: `archive` sets `is_archived`, `cancel` sets `canceled_at`
/// and a status of `canceled` when its enumeration (`statuses`) has one,
/// `unarchive` undoes it. Nothing for an action the fields do not name.
pub(crate) fn done(
    verb: &str,
    item: &Value,
    statuses: &dyn Fn(&str) -> Vec<String>,
) -> Option<Value> {
    let word = verb.split(['-', '_']).next().unwrap_or(verb).to_lowercase();
    let (word, on) = match word.strip_prefix("un") {
        Some(rest) if rest.len() > 3 => (rest.to_owned(), false),
        _ => (word, true),
    };
    let pasts = pasts(&word);
    let fields = item.as_object()?;
    let mut change = Map::new();
    for past in &pasts {
        for name in [format!("is_{past}"), past.clone()] {
            if fields.get(&name).is_some_and(Value::is_boolean) {
                change.insert(name, Value::Bool(on));
            }
        }
        let at = format!("{past}_at");
        if fields
            .get(&at)
            .is_some_and(|v| v.is_string() || v.is_null())
        {
            change.insert(
                at,
                if on {
                    Value::String(now())
                } else {
                    Value::Null
                },
            );
        }
    }
    for field in ["status", "state"].into_iter().filter(|_| on) {
        if !fields.contains_key(field) {
            continue;
        }
        let values = statuses(field);
        let chosen = pasts
            .iter()
            .find_map(|past| values.iter().find(|v| *v == past))
            .or_else(|| {
                pasts
                    .iter()
                    .find_map(|past| values.iter().find(|v| v.starts_with(past.as_str())))
            });
        if let Some(value) = chosen {
            change.insert(field.to_owned(), Value::String(value.clone()));
        }
    }
    (!change.is_empty()).then_some(Value::Object(change))
}

/// The ways a verb says it was done: `canceled` and `cancelled`,
/// `archived`, `verified`, `submitted`, and the word itself, `uploaded`.
fn pasts(word: &str) -> Vec<String> {
    let mut forms = vec![word.to_owned(), format!("{word}ed"), format!("{word}d")];
    if let Some(last) = word.chars().last() {
        forms.push(format!("{word}{last}ed"));
    }
    if let Some(stem) = word.strip_suffix('y') {
        forms.push(format!("{stem}ied"));
    }
    forms
}

/// The time now, as an answer's dates say it: `2026-10-19T08:30:00Z`.
fn now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let rest = seconds % 86_400;
    // Days to a date of the calendar, as Howard Hinnant's `civil_from_days`.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The items of a list's answer: the answer itself, or its field that
/// holds a page of them (`items`, `data`…), else its only list of objects.
pub(crate) fn list_in(body: &mut Value) -> Option<&mut Vec<Value>> {
    if body.is_array() {
        return body.as_array_mut();
    }
    let fields = body.as_object_mut()?;
    let name = LISTS
        .iter()
        .find(|name| fields.get(**name).is_some_and(Value::is_array))
        .map(|name| (*name).to_owned())
        .or_else(|| {
            let mut lists = fields
                .iter()
                .filter(|(_, v)| v.as_array().is_some_and(|a| a.iter().all(Value::is_object)))
                .map(|(name, _)| name.clone());
            let only = lists.next();
            lists.next().is_none().then_some(only).flatten()
        })?;
    fields.get_mut(&name)?.as_array_mut()
}

/// The key an item is known by: its id when it is one of its own, else
/// its id within its collection, `v1/tags#42`, for `42` is many things'.
fn item_key(collection: &str, id: &str) -> String {
    if own(id) {
        id.to_owned()
    } else {
        format!("{collection}#{id}")
    }
}

/// Whether an id is one of its own, long enough to name one thing only.
fn own(id: &str) -> bool {
    id.chars().count() >= 8
}

/// The key of an object wherever it shows, when its id is its own.
fn own_key(object: &Value) -> Option<&str> {
    object.get("id")?.as_str().filter(|id| own(id))
}

/// The field that names an item: the parameter's own when the item has
/// it, `slug` for `{slug}`, else `id`.
fn id_field<'a>(item: &Value, param: &'a str) -> &'a str {
    if item.get(param).is_some() {
        param
    } else {
        "id"
    }
}

/// The id of an item, as a path says it.
fn id_of(item: &Value, param: &str) -> String {
    item.get(id_field(item, param))
        .map(shown)
        .unwrap_or_default()
}

/// A value as a path or a query says it: `42`, `true`, `demo`.
fn shown(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Gives an answer made for no item in particular the id asked for.
fn own_id(answer: &mut Value, param: &str, id: &str) {
    let field = id_field(answer, param);
    let Some(slot) = answer.get_mut(field) else {
        return;
    };
    let asked = match slot {
        Value::Number(_) => id.parse::<i64>().ok().map(Value::from),
        Value::String(_) | Value::Null => Some(Value::String(id.to_owned())),
        _ => None,
    };
    if let Some(asked) = asked {
        *slot = asked;
    }
}

/// Lays `given` over `item`: each field `item` has takes the value given
/// when of the same kind, or when either is null; an object, or a list of
/// them, given ids (`perks: [ids]`, also as `perk_ids`, and an
/// object as its `_id`) takes objects of these ids, shaped as the first
/// made, for what is held of them to be laid over them; a list of objects
/// takes the objects given, each over the first made, with ids of its
/// own. What `item` has not is left out, for the answer to keep to its
/// schema.
fn overlay(item: &mut Value, given: &Value, place: &str) {
    match (&mut *item, given) {
        (Value::Object(fields), Value::Object(given)) => {
            for (name, value) in given {
                let place = format!("{place}.{name}");
                if let Some(slot) = fields.get_mut(name) {
                    overlay(slot, value, &place);
                }
                // An id given for an object the answer holds: `product`
                // for `product_id`, `perks` for `perk_ids`.
                let named = name
                    .strip_suffix("_ids")
                    .map(|stem| format!("{stem}s"))
                    .or_else(|| name.strip_suffix("_id").map(str::to_owned));
                if let Some(slot) = named.and_then(|named| fields.get_mut(&named))
                    && (slot.is_object() || slot.is_array())
                {
                    overlay(slot, value, &place);
                }
            }
        }
        (Value::Array(made), Value::Array(given)) => {
            let first = made.first().filter(|f| f.is_object()).cloned();
            *made = given
                .iter()
                .enumerate()
                .map(|(i, one)| match &first {
                    Some(first) => {
                        let mut shaped = first.clone();
                        overlay(&mut shaped, one, &format!("{place}[{i}]"));
                        if one.is_object()
                            && one.get("id").is_none()
                            && let Some(id) = shaped.get_mut("id")
                        {
                            *id = fresh(id, place, i);
                        }
                        shaped
                    }
                    None => one.clone(),
                })
                .collect();
        }
        // An id for an object: the object of that id.
        (Value::Object(fields), Value::String(_) | Value::Number(_)) => {
            if let Some(id) = fields.get_mut("id") {
                *id = given.clone();
            }
        }
        (item, given) => {
            let same_kind = std::mem::discriminant(&*item) == std::mem::discriminant(given);
            if same_kind || item.is_null() || given.is_null() {
                *item = given.clone();
            }
        }
    }
}

/// Whether two objects are of two kinds of a union: `chat` and
/// `text` perks.
fn other_kind(made: &Value, item: &Value) -> bool {
    ["type", "kind"].iter().any(|field| {
        let (made, its) = (made.get(*field), item.get(*field));
        made.is_some_and(Value::is_string) && its.is_some_and(Value::is_string) && made != its
    })
}

/// Lays an item the page made over one made for its place: over it as
/// [`overlay`] does, or in its stead when it is of another kind with the
/// same fields, a `chat` perk where a `text` one was made. One of
/// another kind and shape is left out: in its stead, it would not be what
/// the place's schema holds.
fn laid(made: &mut Value, item: &Value, place: &str) {
    if !other_kind(made, item) {
        overlay(made, item, place);
        return;
    }
    let fields = |v: &Value| {
        v.as_object()
            .map(|o| o.keys().cloned().collect::<BTreeSet<_>>())
    };
    if fields(made) == fields(item) {
        *made = item.clone();
    }
}

/// What a change gives merged into those given before.
fn merge(into: &mut Value, given: &Value) {
    if let (Value::Object(fields), Value::Object(given)) = (&mut *into, given) {
        for (name, value) in given {
            merge(fields.entry(name.clone()).or_insert(Value::Null), value);
        }
        return;
    }
    *into = given.clone();
}

/// An id for the `n`th item made in `key`, of the kind its slot holds.
fn fresh(slot: &Value, key: &str, n: usize) -> Value {
    match slot {
        Value::Number(_) => json!(1001 + n),
        Value::String(text) if !text.is_empty() && !looks_like_uuid(text) => {
            Value::String(format!("{text}-{}", n + 1))
        }
        _ => Value::String(uuid(&format!("{key}+{n}"))),
    }
}

fn looks_like_uuid(text: &str) -> bool {
    text.len() == 36
        && text.chars().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

/// Narrows a list to what its query asks of its items' fields,
/// `?organization_id=…`, when some item has it: a value no item has is
/// one the items made could not know of, and narrows nothing.
fn narrow(items: &mut Vec<Value>, query: &[(String, String)]) -> bool {
    let mut narrowed = false;
    let names: BTreeSet<&str> = query.iter().map(|(name, _)| name.as_str()).collect();
    for name in names {
        let wanted: Vec<&str> = query
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, value)| value.as_str())
            .collect();
        let fits = |item: &Value| match item.get(name) {
            Some(Value::Array(values)) => {
                values.iter().any(|v| wanted.contains(&shown(v).as_str()))
            }
            Some(value) => wanted.contains(&shown(value).as_str()),
            None => false,
        };
        if items.iter().any(fits) && !items.iter().all(fits) {
            items.retain(fits);
            narrowed = true;
        }
    }
    narrowed
}

/// The counts of a list's answer made to hold its items: `total_count`,
/// in the answer or in its `pagination`, and a page at least when there
/// is an item.
fn recount(body: &mut Value, count: usize, depth: usize) {
    let Some(fields) = body.as_object_mut() else {
        return;
    };
    for (name, value) in fields.iter_mut() {
        let lower = name.to_lowercase();
        let pages = ["max_page", "total_pages"].contains(&lower.as_str());
        match value {
            Value::Number(n) if pages => {
                if count > 0 && n.as_u64() == Some(0) {
                    *value = json!(1);
                }
            }
            Value::Number(_) if COUNTS.contains(&lower.as_str()) => *value = json!(count),
            Value::Object(_) if depth == 0 => recount(value, count, 1),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Spec {
        Spec::read(
            r##"{"paths": {
                "/v1/products/": {"get": {}, "post": {}},
                "/v1/products/{id}": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Product"}}}}}}, "patch": {}, "delete": {}},
                "/v1/products/{id}/perks": {"post": {"responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Product"}}}}}}},
                "/v1/products/{id}/refund": {"post": {}},
                "/v1/members/": {"get": {"responses": {"200": {"content": {"application/json": {"schema": {"type": "object", "properties": {"items": {"type": "array", "items": {"$ref": "#/components/schemas/Member"}}}}}}}}}},
                "/v1/teams/{id}/members": {"post": {"responses": {"201": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Member"}}}}}}},
                "/v1/users/me": {"get": {}, "patch": {}},
                "/v1/metrics": {"get": {}}},
              "components": {"schemas": {
                "Product": {"type": "object", "properties": {"id": {"type": "string"}}},
                "Member": {"type": "object", "properties": {"id": {"type": "string"}, "team_id": {"type": "string"}}}}}}"##,
        )
        .unwrap()
    }

    #[test]
    fn roles_are_read_from_the_paths() {
        let spec = spec();
        let roles: Vec<(String, Role, Option<usize>)> = roles(&spec)
            .into_iter()
            .zip(&spec.operations)
            .map(|((role, list), o)| (format!("{} {}", o.method, o.path), role, list))
            .collect();
        let of = |name: &str| roles.iter().find(|(n, ..)| n == name).cloned().unwrap();
        let at = |name: &str| roles.iter().position(|(n, ..)| n == name);
        let id = || "id".to_owned();
        assert_eq!(of("GET /v1/products/").1, Role::List { param: id() });
        assert_eq!(
            of("POST /v1/products/").1,
            Role::Create {
                param: id(),
                elsewhere: false
            }
        );
        assert_eq!(of("POST /v1/products/").2, at("GET /v1/products/"));
        assert_eq!(
            of("GET /v1/products/{id}").1,
            Role::Read { param: Some(id()) }
        );
        assert_eq!(of("PATCH /v1/products/{id}").2, at("GET /v1/products/"));
        assert_eq!(
            of("DELETE /v1/products/{id}").1,
            Role::Delete { param: id() }
        );
        assert_eq!(
            of("POST /v1/products/{id}/perks").1,
            Role::Act {
                param: id(),
                verb: "perks".to_owned()
            }
        );
        assert_eq!(of("POST /v1/products/{id}/refund").1, Role::Other);
        // A list of items with ids, though none is read alone.
        assert_eq!(of("GET /v1/members/").1, Role::List { param: id() });
        assert_eq!(
            of("POST /v1/teams/{id}/members"),
            (
                "POST /v1/teams/{id}/members".to_owned(),
                Role::Create {
                    param: id(),
                    elsewhere: true
                },
                at("GET /v1/members/")
            )
        );
        assert_eq!(of("GET /v1/users/me").1, Role::Read { param: None });
        assert_eq!(of("PATCH /v1/users/me").1, Role::Update { param: None });
        assert_eq!(of("GET /v1/metrics").1, Role::Other);
        let members = spec.find("POST", "/v1/teams/c1/members").unwrap();
        assert_eq!(
            links(
                spec.operations[members].parts(),
                &spec.bound(members, "/v1/teams/c1/members")
            ),
            [("team_id".to_owned(), "c1".to_owned())]
        );
    }

    #[test]
    fn what_is_given_keeps_to_the_shape_made() {
        let mut item = json!({
            "id": "a", "name": "Demo", "price": 12, "tags": [], "canceled_at": null,
            "prices": [{"id": "p", "amount": 1, "currency": "usd"}],
            "perks": [{"id": "b0", "type": "text"}],
            "product_id": "p0", "product": {"id": "p0", "name": "Made"}
        });
        overlay(
            &mut item,
            &json!({
                "name": "Mine", "price": "free", "unknown": 1, "canceled_at": "2026-10-01",
                "prices": [{"amount": 5}, {"amount": 9, "id": "kept"}],
                "perk_ids": ["b7", "b8"], "product_id": "p9"
            }),
            "x",
        );
        assert_eq!(item["name"], "Mine");
        // Not of the kind made: left as made.
        assert_eq!(item["price"], 12);
        assert!(item.get("unknown").is_none());
        assert_eq!(item["canceled_at"], "2026-10-01");
        assert_eq!(item["prices"][0]["amount"], 5);
        assert_eq!(item["prices"][0]["currency"], "usd");
        assert_ne!(item["prices"][0]["id"], "p");
        assert_eq!(item["prices"][1]["id"], "kept");
        assert_eq!(
            item["perks"],
            json!([{"id": "b7", "type": "text"}, {"id": "b8", "type": "text"}])
        );
        assert_eq!(item["product_id"], "p9");
        assert_eq!(item["product"], json!({"id": "p9", "name": "Made"}));
    }

    #[test]
    fn an_item_of_another_kind_is_laid_whole_or_not_at_all() {
        let mut made = json!({"id": "b0", "type": "text", "properties": {"note": "Made"}});
        let item = json!({"id": "b9", "type": "chat", "properties": {"room_id": "g"}});
        laid(&mut made, &item, "x");
        assert_eq!(made, item);
        // Another shape: what the place holds stays as made.
        let mut flat = json!({"id": "b0", "type": "text", "note": "Made"});
        laid(&mut flat, &item, "x");
        assert_eq!(flat["type"], "text");
    }

    #[test]
    fn a_list_is_narrowed_only_by_what_some_item_has() {
        let made = || {
            vec![
                json!({"id": "1", "org": "a", "archived": false}),
                json!({"id": "2", "org": "a", "archived": true}),
            ]
        };
        let ask = |query: &[(&str, &str)]| {
            let mut items = made();
            let query: Vec<(String, String)> = query
                .iter()
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect();
            narrow(&mut items, &query);
            items.len()
        };
        assert_eq!(ask(&[("archived", "false")]), 1);
        assert_eq!(ask(&[("org", "a")]), 2);
        assert_eq!(ask(&[("org", "elsewhere")]), 2);
        assert_eq!(ask(&[("limit", "10"), ("id", "1"), ("id", "2")]), 2);
        assert_eq!(ask(&[("id", "2")]), 1);
    }

    #[test]
    fn created_changed_and_deleted_items_are_held_until_forgotten() {
        let mut records = Records::default();
        let seed = "seed-0001";
        let seeds = || vec![json!({"id": "seed-0001", "name": "Seed"})];
        let template = || Some(json!({"id": "t", "name": "Made", "count": 0}));
        let list = |records: &mut Records| {
            let mut body =
                json!({"items": seeds(), "pagination": {"total_count": 1, "max_page": 1}});
            let held = records.list("v1/things", "Thing", "id", &mut body, template, &[], &[]);
            (body, held)
        };
        assert!(!list(&mut records).1);

        let (answer, record) = records.create(
            "v1/things",
            "Thing",
            "id",
            Some(&json!({"name": "Mine", "secret": "no"})),
            Some(json!({"id": "t", "name": "Made"})),
            template,
            &[],
        );
        let answer = answer.unwrap();
        assert_eq!(record["id"], answer["id"]);
        assert_eq!(answer["name"], "Mine");
        let id = answer["id"].as_str().unwrap().to_owned();
        assert_ne!(id, "t");
        let (body, held) = list(&mut records);
        assert!(held);
        assert_eq!(body["items"][0]["id"], id.as_str());
        assert_eq!(body["items"][0]["count"], 0);
        assert_eq!(body["pagination"]["total_count"], 2);
        let made = || Some(json!({"id": "x", "name": "Made", "about": "Text"}));
        assert_eq!(
            records
                .read("v1/things", Some("id"), &id, seeds, made())
                .unwrap()
                .0
                .unwrap()["name"],
            "Mine"
        );
        // A seed reads as it was listed; an id never seen as its own.
        assert_eq!(
            records
                .read("v1/things", Some("id"), seed, seeds, made())
                .unwrap(),
            (
                Some(json!({"id": seed, "name": "Seed", "about": "Text"})),
                false
            )
        );
        assert_eq!(
            records
                .read("v1/things", Some("id"), "other", seeds, made())
                .unwrap()
                .0
                .unwrap()["id"],
            "other"
        );

        let changed = records
            .update(
                "v1/things",
                Some("id"),
                seed,
                Some(&json!({"name": "Renamed"})),
                None,
                seeds,
                made(),
            )
            .unwrap();
        assert_eq!(
            changed,
            (
                Some(json!({"id": seed, "name": "Renamed", "about": "Text"})),
                true
            )
        );
        assert_eq!(list(&mut records).0["items"][1]["name"], "Renamed");
        // Renamed wherever it shows.
        let mut elsewhere =
            json!({"orders": [{"id": "order-001", "thing": {"id": seed, "name": "Made"}}]});
        assert!(records.lay(&mut elsewhere));
        assert_eq!(elsewhere["orders"][0]["thing"]["name"], "Renamed");

        records.delete("v1/things", &id).unwrap();
        assert_eq!(records.delete("v1/things", &id), Err(Gone));
        assert_eq!(
            records.read("v1/things", Some("id"), &id, seeds, made()),
            Err(Gone)
        );
        let (body, _) = list(&mut records);
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(
            records.held(),
            [Held {
                path: "/v1/things".to_owned(),
                created: 0,
                changed: 1,
                deleted: 1
            }]
        );
        // An item changed and read by its id alone, as a program does.
        records
            .change_item(seed, &json!({"name": "By hand"}))
            .unwrap();
        assert_eq!(records.item(seed).unwrap()["name"], "By hand");
        assert_eq!(records.item("never-seen"), None);
        records.delete_item(seed).unwrap();
        assert_eq!(records.item(seed), None);
        let mut elsewhere = json!({"things": [{"id": seed}, {"id": "other-0001"}]});
        assert!(records.lay(&mut elsewhere));
        assert_eq!(elsewhere["things"], json!([{"id": "other-0001"}]));

        records.forget();
        assert!(records.held().is_empty());
        assert!(!list(&mut records).1);
    }

    #[test]
    fn a_thing_alone_is_changed_where_it_is_read() {
        let mut records = Records::default();
        let me = || Some(json!({"name": "Ada", "email": "ada@example.com"}));
        records
            .update(
                "v1/users/me",
                None,
                "",
                Some(&json!({"name": "Grace"})),
                None,
                Vec::new,
                me(),
            )
            .unwrap();
        let (read, held) = records
            .read("v1/users/me", None, "", Vec::new, me())
            .unwrap();
        assert!(held);
        assert_eq!(read.unwrap()["name"], "Grace");
    }

    #[test]
    fn what_an_action_names_is_done_to_its_item() {
        let item = json!({"id": "s", "status": "active", "canceled_at": null, "is_archived": false, "is_uploaded": false});
        let statuses = |field: &str| {
            assert_eq!(field, "status");
            vec!["active".to_owned(), "canceled".to_owned()]
        };
        let cancel = done("cancel", &item, &statuses).unwrap();
        assert_eq!(cancel["status"], "canceled");
        assert!(cancel["canceled_at"].as_str().unwrap().starts_with("20"));
        assert_eq!(
            done("archive", &item, &statuses),
            Some(json!({"is_archived": true}))
        );
        assert_eq!(
            done("unarchive", &item, &statuses),
            Some(json!({"is_archived": false}))
        );
        assert_eq!(
            done("uploaded", &item, &statuses),
            Some(json!({"is_uploaded": true}))
        );
        assert_eq!(done("perks", &item, &statuses), None);
        assert_eq!(now().len(), "2026-10-19T08:30:00Z".len());
    }

    #[test]
    fn a_new_id_is_of_the_kind_made() {
        assert_eq!(fresh(&json!(7), "k", 0), json!(1001));
        assert_eq!(fresh(&json!("demo"), "k", 1), json!("demo-2"));
        let made = fresh(&json!(uuid("x")), "k", 0);
        assert!(looks_like_uuid(made.as_str().unwrap()));
        assert_ne!(made, fresh(&json!(uuid("x")), "k", 1));
    }
}

//! Shared type dictionary for compact plan schemas.
//!
//! A compact plan schema inlines every input type at every use, so a scalar
//! filter repeats once per filterable field and an entity filter repeats once
//! per relationship path. [`AiSharedTypePlanSchema`] is a lossless rendering
//! of the same schema in which each repeated subschema is defined once in a
//! dictionary and referenced by name. It is a presentation of an existing
//! contract: it carries no authority, changes no fingerprint, and expands
//! back to the exact schema it was compiled from.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{AiError, canonical_json::canonical_json_value};

/// JSON-pointer prefix of one shared type reference.
pub const AI_PLAN_SCHEMA_TYPE_REFERENCE_PREFIX: &str = "#/types/";

/// Key under which [`AiSharedTypePlanSchema::to_model_schema`] publishes the
/// dictionary.
pub const AI_PLAN_SCHEMA_TYPES_KEY: &str = "types";

/// Smallest inlined subschema, in bytes, worth defining once.
const MINIMUM_SHARED_TYPE_BYTES: usize = 128;
const MAXIMUM_SCHEMA_DEPTH: usize = 128;
const PLACEHOLDER_KEY: &str = "\u{0}node";
const DESCRIPTION_KEY: &str = "description";

/// One compact plan schema with its repeated subschemas defined once.
///
/// [`Self::schema`] is the root schema; a `{"$ref": "#/types/<Name>"}` in it,
/// or in a dictionary entry, stands for [`Self::type_definition`] of that
/// name. A reference may carry a sibling `description` for that one use.
/// Nullable and list wrappers stay inline around the reference, so a
/// dictionary entry is always the type itself.
/// Names come from the GraphQL input types the subschemas were generated
/// from when the caller supplies them, and are otherwise `Type<n>`.
///
/// The value proves nothing about authority. It is a rendering of the schema
/// passed to [`Self::compile`], and [`Self::expand`] returns that schema.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiSharedTypePlanSchema {
    schema: Value,
    types: BTreeMap<String, Value>,
}

impl AiSharedTypePlanSchema {
    /// Compiles the shared rendering of one plan schema.
    ///
    /// `type_names` maps [`Self::type_name_key`] of a subschema to the public
    /// name to give it. A subschema without an entry gets a positional name.
    /// Compilation is deterministic.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] when the schema is nested
    /// deeper than the supported bound, already uses the reserved dictionary
    /// key or reference prefix, or contains a reserved placeholder key.
    pub fn compile(schema: &Value, type_names: &BTreeMap<String, String>) -> Result<Self, AiError> {
        if schema
            .as_object()
            .is_some_and(|root| root.contains_key(AI_PLAN_SCHEMA_TYPES_KEY))
        {
            return Err(invalid("plan schema already declares a type dictionary"));
        }
        let mut arena = Arena::default();
        let (root, _) = arena.intern(schema, 0)?;
        let hoisted = arena.hoisted(root);
        let names = arena.names(&hoisted, type_names);
        let mut types = BTreeMap::new();
        for (id, name) in &names {
            types.insert(name.clone(), arena.emit(*id, &names));
        }
        Ok(Self {
            schema: arena.emit(root, &names),
            types,
        })
    }

    /// Stable lookup key of one subschema for the `type_names` map of
    /// [`Self::compile`]. A top-level `description` does not affect the key,
    /// because a description belongs to one use of a type.
    pub fn type_name_key(schema: &Value) -> String {
        let mut schema = schema.clone();
        if let Some(object) = schema.as_object_mut() {
            object.remove(DESCRIPTION_KEY);
        }
        let encoded = serde_json::to_vec(&canonical_json_value(schema))
            .expect("a JSON value always serializes");
        hex::encode(Sha256::digest(encoded))
    }

    /// Root schema, with repeated subschemas replaced by references.
    pub const fn schema(&self) -> &Value {
        &self.schema
    }

    /// Every shared type by name, in name order.
    pub const fn types(&self) -> &BTreeMap<String, Value> {
        &self.types
    }

    /// Definition of one shared type.
    pub fn type_definition(&self, name: &str) -> Option<&Value> {
        self.types.get(name)
    }

    /// Root schema with the complete dictionary under
    /// [`AI_PLAN_SCHEMA_TYPES_KEY`]. The result is one self-contained JSON
    /// document: every `#/types/<Name>` reference resolves inside it.
    pub fn to_model_schema(&self) -> Value {
        let mut schema = self.schema.clone();
        if self.types.is_empty() {
            return schema;
        }
        if let Some(root) = schema.as_object_mut() {
            root.insert(
                AI_PLAN_SCHEMA_TYPES_KEY.to_owned(),
                Value::Object(
                    self.types
                        .iter()
                        .map(|(name, definition)| (name.clone(), definition.clone()))
                        .collect(),
                ),
            );
        }
        schema
    }

    /// Inlines every shared type and returns the schema this value was
    /// compiled from.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] for a reference to an absent
    /// type or a reference cycle. Neither can occur in a compiled value.
    pub fn expand(&self) -> Result<Value, AiError> {
        self.expand_value(&self.schema, 0)
    }

    fn expand_value(&self, value: &Value, depth: usize) -> Result<Value, AiError> {
        if depth > MAXIMUM_SCHEMA_DEPTH * 2 {
            return Err(invalid("plan schema type references are too deep"));
        }
        match value {
            Value::Object(object) => {
                if let Some(name) = shared_reference(object) {
                    let definition = self
                        .types
                        .get(name)
                        .ok_or_else(|| invalid("plan schema references an absent type"))?;
                    let mut expanded = self.expand_value(definition, depth + 1)?;
                    if let (Some(target), Some(description)) =
                        (expanded.as_object_mut(), object.get(DESCRIPTION_KEY))
                    {
                        target.insert(DESCRIPTION_KEY.to_owned(), description.clone());
                    }
                    return Ok(expanded);
                }
                let mut expanded = Map::new();
                for (key, child) in object {
                    expanded.insert(key.clone(), self.expand_value(child, depth + 1)?);
                }
                Ok(Value::Object(expanded))
            }
            Value::Array(values) => values
                .iter()
                .map(|child| self.expand_value(child, depth + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            scalar => Ok(scalar.clone()),
        }
    }
}

fn shared_reference(object: &Map<String, Value>) -> Option<&str> {
    let name = object
        .get("$ref")?
        .as_str()?
        .strip_prefix(AI_PLAN_SCHEMA_TYPE_REFERENCE_PREFIX)?;
    object
        .keys()
        .all(|key| key == "$ref" || key == DESCRIPTION_KEY)
        .then_some(name)
}

fn invalid(message: &str) -> AiError {
    AiError::InvalidConfiguration(message.to_owned())
}

/// One distinct subschema. Children are held by placeholder in `shape`.
struct Node<'a> {
    shape: Value,
    children: Vec<usize>,
    raw: &'a Value,
    raw_bytes: usize,
    height: usize,
    shareable: bool,
}

#[derive(Default)]
struct Arena<'a> {
    nodes: Vec<Node<'a>>,
    interned: HashMap<Vec<u8>, usize>,
}

impl<'a> Arena<'a> {
    /// Interns one schema-position value and returns its node and the
    /// description of this particular use.
    fn intern(
        &mut self,
        value: &'a Value,
        depth: usize,
    ) -> Result<(usize, Option<&'a Value>), AiError> {
        if depth > MAXIMUM_SCHEMA_DEPTH {
            return Err(invalid("plan schema is nested too deeply"));
        }
        let mut children = Vec::new();
        let (shape, description, shareable) = match value {
            Value::Object(object) => {
                if object.contains_key(PLACEHOLDER_KEY)
                    || object
                        .get("$ref")
                        .and_then(Value::as_str)
                        .is_some_and(|target| {
                            target.starts_with(AI_PLAN_SCHEMA_TYPE_REFERENCE_PREFIX)
                        })
                {
                    return Err(invalid("plan schema uses a reserved key"));
                }
                let mut shape = Map::new();
                for (key, child) in object {
                    if key == DESCRIPTION_KEY {
                        continue;
                    }
                    let child = match (key.as_str(), child) {
                        ("properties" | "patternProperties", Value::Object(members)) => {
                            let mut shaped = Map::new();
                            for (name, member) in members {
                                shaped.insert(
                                    name.clone(),
                                    self.child(member, depth, &mut children)?,
                                );
                            }
                            Value::Object(shaped)
                        }
                        (
                            "items" | "additionalProperties" | "propertyNames" | "not" | "contains",
                            Value::Object(_),
                        ) => self.child(child, depth, &mut children)?,
                        ("anyOf" | "oneOf" | "allOf" | "prefixItems", Value::Array(members)) => {
                            Value::Array(
                                members
                                    .iter()
                                    .map(|member| self.child(member, depth, &mut children))
                                    .collect::<Result<_, _>>()?,
                            )
                        }
                        _ => child.clone(),
                    };
                    shape.insert(key.clone(), child);
                }
                // A bare reference or an empty schema is never worth sharing.
                // Nor is a nullable or list wrapper: sharing the type it
                // wraps keeps that type under its own name, and the wrapper
                // around a reference is already small.
                let shareable =
                    !object.contains_key("$ref") && !shape.is_empty() && !thin_wrapper(&shape);
                (Value::Object(shape), object.get(DESCRIPTION_KEY), shareable)
            }
            other => (other.clone(), None, false),
        };
        let key = serde_json::to_vec(&canonical_json_value(shape.clone()))
            .map_err(|_| invalid("plan schema cannot be serialized"))?;
        if let Some(existing) = self.interned.get(&key) {
            return Ok((*existing, description));
        }
        let shape_bytes = key.len();
        let raw_bytes = children.iter().fold(shape_bytes, |total, child| {
            total.saturating_add(self.nodes[*child].raw_bytes)
        });
        let height = children
            .iter()
            .map(|child| self.nodes[*child].height + 1)
            .max()
            .unwrap_or(0);
        let id = self.nodes.len();
        self.nodes.push(Node {
            shape,
            children,
            raw: value,
            raw_bytes,
            height,
            shareable,
        });
        self.interned.insert(key, id);
        Ok((id, description))
    }

    fn child(
        &mut self,
        value: &'a Value,
        depth: usize,
        children: &mut Vec<usize>,
    ) -> Result<Value, AiError> {
        let (id, description) = self.intern(value, depth + 1)?;
        children.push(id);
        let mut placeholder = Map::new();
        placeholder.insert(PLACEHOLDER_KEY.to_owned(), Value::from(id));
        if let Some(description) = description {
            placeholder.insert(DESCRIPTION_KEY.to_owned(), description.clone());
        }
        Ok(Value::Object(placeholder))
    }

    /// Decides which nodes are defined once. A node is shared when the
    /// rendering would otherwise emit it at least twice and it is large
    /// enough to be worth a reference. Sharing a node stops its children from
    /// being counted once per use of that node.
    fn hoisted(&self, root: usize) -> BTreeSet<usize> {
        let mut order = (0..self.nodes.len()).collect::<Vec<_>>();
        order.sort_by(|left, right| {
            self.nodes[*right]
                .height
                .cmp(&self.nodes[*left].height)
                .then_with(|| left.cmp(right))
        });
        let mut uses = vec![0_u64; self.nodes.len()];
        uses[root] = 1;
        let mut hoisted = BTreeSet::new();
        for id in order {
            let node = &self.nodes[id];
            let shared = id != root
                && node.shareable
                && uses[id] >= 2
                && node.raw_bytes >= MINIMUM_SHARED_TYPE_BYTES;
            if shared {
                hoisted.insert(id);
            }
            let emissions = if shared { 1 } else { uses[id] };
            for child in &node.children {
                uses[*child] = uses[*child].saturating_add(emissions);
            }
        }
        hoisted
    }

    fn names(
        &self,
        hoisted: &BTreeSet<usize>,
        type_names: &BTreeMap<String, String>,
    ) -> BTreeMap<usize, String> {
        let mut names = BTreeMap::new();
        let mut used = BTreeSet::new();
        let mut positional = 0_usize;
        // Node identifiers are assigned children-first in document order, so
        // this order is deterministic for one schema.
        for id in hoisted {
            let base = self.suggested_name(*id, type_names).unwrap_or_else(|| {
                positional += 1;
                format!("Type{positional}")
            });
            let mut name = base.clone();
            let mut suffix = 1_usize;
            while !used.insert(name.clone()) {
                suffix += 1;
                name = format!("{base}_{suffix}");
            }
            names.insert(*id, name);
        }
        names
    }

    fn suggested_name(&self, id: usize, type_names: &BTreeMap<String, String>) -> Option<String> {
        type_names
            .get(&AiSharedTypePlanSchema::type_name_key(self.nodes[id].raw))
            .filter(|name| valid_type_name(name))
            .cloned()
    }

    fn emit(&self, id: usize, names: &BTreeMap<usize, String>) -> Value {
        self.emit_shape(&self.nodes[id].shape, names)
    }

    fn emit_shape(&self, shape: &Value, names: &BTreeMap<usize, String>) -> Value {
        match shape {
            Value::Object(object) => {
                if let Some(child) = object
                    .get(PLACEHOLDER_KEY)
                    .and_then(Value::as_u64)
                    .and_then(|child| usize::try_from(child).ok())
                {
                    let mut emitted = match names.get(&child) {
                        Some(name) => {
                            let mut reference = Map::new();
                            reference.insert(
                                "$ref".to_owned(),
                                Value::String(format!(
                                    "{AI_PLAN_SCHEMA_TYPE_REFERENCE_PREFIX}{name}"
                                )),
                            );
                            Value::Object(reference)
                        }
                        None => self.emit(child, names),
                    };
                    if let (Some(target), Some(description)) =
                        (emitted.as_object_mut(), object.get(DESCRIPTION_KEY))
                    {
                        target.insert(DESCRIPTION_KEY.to_owned(), description.clone());
                    }
                    return emitted;
                }
                Value::Object(
                    object
                        .iter()
                        .map(|(key, child)| (key.clone(), self.emit_shape(child, names)))
                        .collect(),
                )
            }
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .map(|child| self.emit_shape(child, names))
                    .collect(),
            ),
            scalar => scalar.clone(),
        }
    }
}

/// Whether a shape is only `anyOf [T, U]` or an array of one item type.
fn thin_wrapper(shape: &Map<String, Value>) -> bool {
    let two_alternatives = shape.len() == 1
        && shape
            .get("anyOf")
            .and_then(Value::as_array)
            .is_some_and(|members| members.len() == 2);
    let list = shape.get("type").and_then(Value::as_str) == Some("array")
        && shape
            .get("items")
            .is_some_and(|items| items.get(PLACEHOLDER_KEY).is_some());
    two_alternatives || list
}

fn valid_type_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn string_filter() -> Value {
        json!({
            "type": "object",
            "properties": {
                "eq": {"anyOf": [{"type": "string", "maxLength": 65536}, {"type": "null"}]},
                "ne": {"anyOf": [{"type": "string", "maxLength": 65536}, {"type": "null"}]},
                "contains": {"anyOf": [{"type": "string", "maxLength": 65536}, {"type": "null"}]},
                "in": {"anyOf": [
                    {"type": "array", "maxItems": 100,
                     "items": {"type": "string", "maxLength": 65536}},
                    {"type": "null"}
                ]}
            },
            "required": [],
            "additionalProperties": false
        })
    }

    fn described(mut schema: Value, description: &str) -> Value {
        schema
            .as_object_mut()
            .unwrap()
            .insert("description".to_owned(), json!(description));
        schema
    }

    fn nullable(schema: Value) -> Value {
        json!({"anyOf": [schema, {"type": "null"}]})
    }

    // The generator puts a field's description on the type and wraps the
    // result when the field is nullable.
    fn where_input() -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": nullable(described(string_filter(), "Name of the record.")),
                "code": nullable(described(string_filter(), "Short code.")),
                "notes": nullable(described(string_filter(), "Free text.")),
            },
            "required": [],
            "additionalProperties": false
        })
    }

    fn plan_schema() -> Value {
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "arguments": {
                    "type": "object",
                    "properties": {
                        "where": nullable(described(where_input(), "Root filter."))
                    },
                    "required": [],
                    "additionalProperties": false
                },
                "relationshipArguments": {
                    "type": "object",
                    "properties": {
                        "children": described(json!({
                            "type": "object",
                            "properties": {
                                "where": nullable(described(where_input(), "Child filter."))
                            },
                            "required": [],
                            "additionalProperties": false
                        }), "Child records."),
                    },
                    "required": [],
                    "additionalProperties": false
                }
            },
            "required": ["arguments"],
            "additionalProperties": false
        })
    }

    fn names() -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                AiSharedTypePlanSchema::type_name_key(&string_filter()),
                "StringFilter".to_owned(),
            ),
            (
                AiSharedTypePlanSchema::type_name_key(&where_input()),
                "RecordWhereInput".to_owned(),
            ),
        ])
    }

    #[test]
    fn repeated_types_are_defined_once_and_expand_to_the_original_schema() {
        let original = plan_schema();
        let shared = AiSharedTypePlanSchema::compile(&original, &names()).unwrap();

        assert_eq!(shared.expand().unwrap(), original);
        assert!(shared.type_definition("StringFilter").is_some());
        assert!(shared.type_definition("RecordWhereInput").is_some());
        assert_eq!(shared.types().len(), 2);
        let encoded = serde_json::to_string(&shared.to_model_schema()).unwrap();
        // The scalar filter body appears in its one definition only.
        assert_eq!(encoded.matches("\"contains\"").count(), 1);
        assert!(encoded.len() < serde_json::to_string(&original).unwrap().len());
    }

    #[test]
    fn a_reference_keeps_the_description_of_its_own_use() {
        let shared = AiSharedTypePlanSchema::compile(&plan_schema(), &names()).unwrap();
        let record = shared.type_definition("RecordWhereInput").unwrap();
        assert_eq!(
            record["properties"]["code"],
            json!({"anyOf": [
                {"$ref": "#/types/StringFilter", "description": "Short code."},
                {"type": "null"}
            ]})
        );
        assert_eq!(
            shared.schema()["properties"]["arguments"]["properties"]["where"],
            json!({"anyOf": [
                {"$ref": "#/types/RecordWhereInput", "description": "Root filter."},
                {"type": "null"}
            ]})
        );
    }

    #[test]
    fn nullable_and_list_wrappers_stay_inline_around_the_shared_type() {
        let schema = json!({
            "type": "object",
            "properties": {
                "first": nullable(string_filter()),
                "second": nullable(string_filter()),
                "many": {"type": "array", "maxItems": 4, "items": string_filter()},
                "more": {"type": "array", "maxItems": 4, "items": string_filter()},
            },
            "required": [],
            "additionalProperties": false
        });
        let shared = AiSharedTypePlanSchema::compile(&schema, &names()).unwrap();
        assert_eq!(shared.expand().unwrap(), schema);
        assert_eq!(
            shared
                .types()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["StringFilter"]
        );
        assert_eq!(
            shared.schema()["properties"]["second"],
            json!({"anyOf": [{"$ref": "#/types/StringFilter"}, {"type": "null"}]})
        );
        assert_eq!(
            shared.schema()["properties"]["more"],
            json!({"type": "array", "maxItems": 4, "items": {"$ref": "#/types/StringFilter"}})
        );
    }

    #[test]
    fn unnamed_types_get_deterministic_positional_names() {
        let first = AiSharedTypePlanSchema::compile(&plan_schema(), &BTreeMap::new()).unwrap();
        let second = AiSharedTypePlanSchema::compile(&plan_schema(), &BTreeMap::new()).unwrap();
        assert_eq!(first, second);
        assert!(first.types().keys().all(|name| name.starts_with("Type")));
        assert_eq!(first.expand().unwrap(), plan_schema());
    }

    #[test]
    fn a_schema_without_repetition_is_returned_unchanged() {
        let schema = json!({
            "type": "object",
            "properties": {"arguments": string_filter()},
            "required": ["arguments"],
            "additionalProperties": false
        });
        let shared = AiSharedTypePlanSchema::compile(&schema, &names()).unwrap();
        assert!(shared.types().is_empty());
        assert_eq!(shared.to_model_schema(), schema);
    }

    #[test]
    fn existing_local_definitions_and_references_are_preserved() {
        let json_scalar = json!({"$ref": "#/$defs/canonical_json_1"});
        let schema = json!({
            "type": "object",
            "properties": {
                "first": where_input(),
                "second": where_input(),
                "payload": json_scalar,
            },
            "required": [],
            "additionalProperties": false,
            "$defs": {"canonical_json_1": {"anyOf": [{"type": "null"}, {"type": "string"}]}}
        });
        let shared = AiSharedTypePlanSchema::compile(&schema, &names()).unwrap();
        assert_eq!(shared.expand().unwrap(), schema);
        assert_eq!(shared.schema()["$defs"], schema["$defs"]);
        assert_eq!(
            shared.schema()["properties"]["payload"],
            json!({"$ref": "#/$defs/canonical_json_1"})
        );
    }

    #[test]
    fn reserved_keys_are_refused() {
        for schema in [
            json!({"type": "object", "types": {}}),
            json!({"type": "object", "properties": {"a": {"$ref": "#/types/A"}}}),
            json!({"type": "object", "properties": {"a": {"\u{0}node": 1}}}),
        ] {
            assert!(matches!(
                AiSharedTypePlanSchema::compile(&schema, &BTreeMap::new()),
                Err(AiError::InvalidConfiguration(_))
            ));
        }
    }
}

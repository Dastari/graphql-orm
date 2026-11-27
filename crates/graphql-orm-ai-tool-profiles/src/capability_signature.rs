//! Compact typed signatures of capability plan schemas.
//!
//! A plan schema is the closed JSON Schema a capability's plan is validated
//! against. For a wide entity it is tens or hundreds of kilobytes. An
//! [`AiCapabilitySignature`] renders the same contract as a short typed
//! signature that names its input types, such as
//! `Records(where?: RecordWhereInput|null, page?: PageInput) -> select`, and
//! expands one named type at a time, in pages. It is a presentation only:
//! it carries no authority, changes no fingerprint, and plans are still
//! validated against the unchanged closed schema.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::{AI_PLAN_SCHEMA_TYPE_REFERENCE_PREFIX, AiError, AiSharedTypePlanSchema};

/// Version of the signature rendering and of the type pages it returns.
pub const AI_CAPABILITY_SIGNATURE_VERSION: u16 = 1;

/// Name of the root selection type. A relationship path `p` is expanded as
/// the type `select.p`.
pub const AI_CAPABILITY_SELECTION_TYPE: &str = "select";

/// Default number of fields or values in one type page.
pub const AI_CAPABILITY_TYPE_PAGE_DEFAULT: u16 = 64;

/// Largest number of fields or values in one type page.
pub const AI_CAPABILITY_TYPE_PAGE_MAXIMUM: u16 = 128;

const MAXIMUM_INLINE_ENUM_VALUES: usize = 16;
const MAXIMUM_RENDER_DEPTH: usize = 32;
const MAXIMUM_SELECTION_PREVIEW_FIELDS: usize = 64;

/// Compact typed signature of one capability's plan schema.
///
/// Built from a named [`AiSharedTypePlanSchema`]: every GraphQL input type
/// is a dictionary entry and the signature refers to it by name.
/// [`Self::describe_type`] expands one input type, or one level of the
/// selection tree, in bounded pages.
///
/// The value proves nothing about authority and is not a validation
/// contract. The model's plan is still validated against the closed schema
/// the signature was rendered from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiCapabilitySignature {
    operation: String,
    plan: AiSharedTypePlanSchema,
    arguments: Option<Value>,
    selection_paths: Vec<String>,
    relationship_arguments: BTreeMap<String, Value>,
    relationship_maximum_items: BTreeMap<String, Value>,
    maximum_items: Option<(Value, bool)>,
}

impl AiCapabilitySignature {
    /// Compiles the signature of one operation from its shared rendering.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] for an empty operation name
    /// or a plan schema whose root is not an object schema.
    pub fn compile(
        operation: impl Into<String>,
        plan: AiSharedTypePlanSchema,
    ) -> Result<Self, AiError> {
        let operation = operation.into();
        if operation.is_empty() || operation.chars().any(char::is_control) {
            return Err(invalid("capability signature operation is invalid"));
        }
        let root = plan
            .schema()
            .as_object()
            .ok_or_else(|| invalid("capability plan schema is not an object"))?;
        let properties = root.get("properties").and_then(Value::as_object);
        let property = |name: &str| properties?.get(name);
        let required = required_names(root);
        let selection_paths = property("selections")
            .and_then(|selections| selections.get("items"))
            .and_then(|items| items.get("enum"))
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let path_map = |key: &str| {
            property(key)
                .and_then(|value| value.get("properties"))
                .and_then(Value::as_object)
                .map(|paths| {
                    paths
                        .iter()
                        .map(|(path, schema)| (path.clone(), schema.clone()))
                        .collect::<BTreeMap<_, _>>()
                })
                .unwrap_or_default()
        };
        let relationship_arguments = path_map("relationshipArguments");
        let relationship_maximum_items = path_map("relationshipMaximumItems");
        let maximum_items = property("maximumItems")
            .map(|schema| (schema.clone(), required.contains("maximumItems")));
        Ok(Self {
            operation,
            arguments: property("arguments").cloned(),
            plan,
            selection_paths,
            relationship_arguments,
            relationship_maximum_items,
            maximum_items,
        })
    }

    /// Public operation the signature describes.
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// Shared rendering the signature was compiled from.
    pub const fn plan_schema(&self) -> &AiSharedTypePlanSchema {
        &self.plan
    }

    /// One-line typed signature, for example
    /// `Records(where?: RecordWhereInput|null) -> select`.
    pub fn signature(&self) -> String {
        let arguments = self
            .arguments
            .as_ref()
            .map(|arguments| self.render_fields_inline(arguments, 0))
            .unwrap_or_default();
        let result = if self.selection_paths.is_empty() {
            "fixed"
        } else {
            AI_CAPABILITY_SELECTION_TYPE
        };
        format!("{}({arguments}) -> {result}", self.operation)
    }

    /// Every type name [`Self::describe_type`] expands: the input types in
    /// name order, then the selection types in path order.
    pub fn type_names(&self) -> Vec<String> {
        self.plan
            .types()
            .keys()
            .cloned()
            .chain(self.selection_type_names())
            .collect()
    }

    /// Bounded model-facing summary of this operation: the signature, the
    /// top level of the selection tree, the root result bound and the input
    /// types it names.
    pub fn summary(&self) -> Value {
        let mut summary = Map::new();
        summary.insert("operation".to_owned(), json!(self.operation));
        summary.insert("signature".to_owned(), json!(self.signature()));
        if !self.selection_paths.is_empty() {
            summary.insert("select".to_owned(), json!(self.selection_preview()));
        }
        if let Some((schema, required)) = &self.maximum_items {
            summary.insert(
                "maximumItems".to_owned(),
                json!({
                    "type": self.render(schema, 0),
                    "required": required,
                }),
            );
        }
        let input_types = self.plan.types().keys().cloned().collect::<Vec<_>>();
        if !input_types.is_empty() {
            summary.insert("inputTypes".to_owned(), json!(input_types));
        }
        Value::Object(summary)
    }

    /// Expands one named type: an input type from [`Self::summary`]'s
    /// `inputTypes`, the root selection type [`AI_CAPABILITY_SELECTION_TYPE`],
    /// or the selection type `select.<path>` of one relationship path.
    ///
    /// Returns at most `limit` fields or enum values starting at `offset`,
    /// with `nextOffset` set when more remain. `limit` is clamped to
    /// 1..=[`AI_CAPABILITY_TYPE_PAGE_MAXIMUM`]. Returns `None` for a name the
    /// signature does not define.
    pub fn describe_type(&self, name: &str, offset: u32, limit: u16) -> Option<Value> {
        let limit = usize::from(limit.clamp(1, AI_CAPABILITY_TYPE_PAGE_MAXIMUM));
        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
        if let Some(definition) = self.plan.type_definition(name) {
            return Some(self.describe_input_type(name, definition, offset, limit));
        }
        let prefix = if name == AI_CAPABILITY_SELECTION_TYPE {
            ""
        } else {
            name.strip_prefix(AI_CAPABILITY_SELECTION_TYPE)?
                .strip_prefix('.')?
        };
        self.describe_selection_type(name, prefix, offset, limit)
    }

    fn describe_input_type(
        &self,
        name: &str,
        definition: &Value,
        offset: usize,
        limit: usize,
    ) -> Value {
        let mut page = Map::new();
        page.insert("version".to_owned(), json!(AI_CAPABILITY_SIGNATURE_VERSION));
        page.insert("type".to_owned(), json!(name));
        if let Some(description) = definition.get("description").and_then(Value::as_str) {
            page.insert("description".to_owned(), json!(description));
        }
        if let Some(properties) = definition.get("properties").and_then(Value::as_object) {
            let required = definition
                .as_object()
                .map(required_names)
                .unwrap_or_default();
            let fields = properties
                .iter()
                .map(|(field, schema)| {
                    let mut item = Map::new();
                    item.insert("name".to_owned(), json!(field));
                    item.insert("type".to_owned(), json!(self.render(schema, 0)));
                    item.insert("required".to_owned(), json!(required.contains(field)));
                    if let Some(description) = schema.get("description").and_then(Value::as_str) {
                        item.insert("description".to_owned(), json!(description));
                    }
                    Value::Object(item)
                })
                .collect::<Vec<_>>();
            page.insert("kind".to_owned(), json!("input"));
            insert_page(&mut page, "fields", fields, offset, limit);
        } else if let Some(values) = definition.get("enum").and_then(Value::as_array) {
            page.insert("kind".to_owned(), json!("enum"));
            insert_page(&mut page, "values", values.clone(), offset, limit);
        } else {
            page.insert("kind".to_owned(), json!("scalar"));
            page.insert("definition".to_owned(), json!(self.render(definition, 0)));
        }
        Value::Object(page)
    }

    fn describe_selection_type(
        &self,
        name: &str,
        prefix: &str,
        offset: usize,
        limit: usize,
    ) -> Option<Value> {
        let children = self.selection_children(prefix);
        if children.is_empty() {
            return None;
        }
        let fields = children
            .into_iter()
            .map(|(field, relationship)| {
                let path = join_path(prefix, &field);
                let mut item = Map::new();
                item.insert("name".to_owned(), json!(field));
                if relationship {
                    item.insert(
                        "type".to_owned(),
                        json!(format!("{AI_CAPABILITY_SELECTION_TYPE}.{path}")),
                    );
                    item.insert("relationship".to_owned(), json!(true));
                    if let Some(arguments) = self.relationship_arguments.get(&path) {
                        item.insert(
                            "arguments".to_owned(),
                            json!(format!("({})", self.render_fields_inline(arguments, 0))),
                        );
                    }
                    if let Some(maximum) = self.relationship_maximum_items.get(&path) {
                        item.insert("maximumItems".to_owned(), json!(self.render(maximum, 0)));
                    }
                } else {
                    item.insert("type".to_owned(), json!("scalar"));
                }
                Value::Object(item)
            })
            .collect::<Vec<_>>();
        let mut page = Map::new();
        page.insert("version".to_owned(), json!(AI_CAPABILITY_SIGNATURE_VERSION));
        page.insert("type".to_owned(), json!(name));
        page.insert("kind".to_owned(), json!("selection"));
        page.insert(
            "description".to_owned(),
            json!(
                "Select a scalar by its dotted path from the root selection, for example \
                 relationship.field."
            ),
        );
        insert_page(&mut page, "fields", fields, offset, limit);
        Some(Value::Object(page))
    }

    /// Direct children of one selection prefix, in path order: each name and
    /// whether it is a relationship with selectable paths below it.
    fn selection_children(&self, prefix: &str) -> Vec<(String, bool)> {
        let mut children = BTreeMap::<String, bool>::new();
        for path in &self.selection_paths {
            let rest = if prefix.is_empty() {
                Some(path.as_str())
            } else {
                path.strip_prefix(prefix)
                    .and_then(|rest| rest.strip_prefix('.'))
            };
            let Some(rest) = rest else {
                continue;
            };
            match rest.split_once('.') {
                Some((child, _)) => {
                    children.insert(child.to_owned(), true);
                }
                None => {
                    children.entry(rest.to_owned()).or_insert(false);
                }
            }
        }
        children.into_iter().collect()
    }

    fn selection_type_names(&self) -> Vec<String> {
        if self.selection_paths.is_empty() {
            return Vec::new();
        }
        let mut names = BTreeSet::new();
        for path in &self.selection_paths {
            let segments = path.split('.').collect::<Vec<_>>();
            for depth in 1..segments.len() {
                names.insert(format!(
                    "{AI_CAPABILITY_SELECTION_TYPE}.{}",
                    segments[..depth].join(".")
                ));
            }
        }
        std::iter::once(AI_CAPABILITY_SELECTION_TYPE.to_owned())
            .chain(names)
            .collect()
    }

    /// Top level of the selection tree: scalars by name, relationships as
    /// `name{…}`.
    fn selection_preview(&self) -> String {
        let children = self.selection_children("");
        let total = children.len();
        let mut rendered = children
            .into_iter()
            .take(MAXIMUM_SELECTION_PREVIEW_FIELDS)
            .map(|(name, relationship)| {
                if relationship {
                    format!("{name}{{…}}")
                } else {
                    name
                }
            })
            .collect::<Vec<_>>();
        if total > MAXIMUM_SELECTION_PREVIEW_FIELDS {
            rendered.push(format!(
                "…(+{} more; describe type {AI_CAPABILITY_SELECTION_TYPE})",
                total - MAXIMUM_SELECTION_PREVIEW_FIELDS
            ));
        }
        rendered.join(", ")
    }

    fn render_fields_inline(&self, schema: &Value, depth: usize) -> String {
        let schema = self.resolve(schema);
        let Some(object) = schema.as_object() else {
            return self.render(schema, depth);
        };
        let required = required_names(object);
        object
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| {
                properties
                    .iter()
                    .map(|(name, member)| {
                        let optional = if required.contains(name) { "" } else { "?" };
                        format!("{name}{optional}: {}", self.render(member, depth + 1))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    }

    /// Follows a shared reference to its definition, for a position whose
    /// fields are rendered inline.
    fn resolve<'a>(&'a self, schema: &'a Value) -> &'a Value {
        schema
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|reference| reference.strip_prefix(AI_PLAN_SCHEMA_TYPE_REFERENCE_PREFIX))
            .and_then(|name| self.plan.type_definition(name))
            .unwrap_or(schema)
    }

    fn render(&self, schema: &Value, depth: usize) -> String {
        if depth > MAXIMUM_RENDER_DEPTH {
            return "JSON".to_owned();
        }
        let Some(object) = schema.as_object() else {
            return match schema {
                Value::Bool(true) => "JSON".to_owned(),
                Value::Bool(false) => "never".to_owned(),
                _ => "JSON".to_owned(),
            };
        };
        if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
            return reference
                .strip_prefix(AI_PLAN_SCHEMA_TYPE_REFERENCE_PREFIX)
                .map_or_else(|| "JSON".to_owned(), str::to_owned);
        }
        for key in ["anyOf", "oneOf"] {
            if let Some(members) = object.get(key).and_then(Value::as_array) {
                return members
                    .iter()
                    .map(|member| self.render(member, depth + 1))
                    .collect::<Vec<_>>()
                    .join("|");
            }
        }
        if let Some(value) = object.get("const") {
            return value.to_string();
        }
        if let Some(values) = object.get("enum").and_then(Value::as_array) {
            let mut rendered = values
                .iter()
                .take(MAXIMUM_INLINE_ENUM_VALUES)
                .map(Value::to_string)
                .collect::<Vec<_>>();
            if values.len() > MAXIMUM_INLINE_ENUM_VALUES {
                rendered.push(format!("…(+{})", values.len() - MAXIMUM_INLINE_ENUM_VALUES));
            }
            return rendered.join("|");
        }
        match object.get("type") {
            Some(Value::Array(types)) => types
                .iter()
                .filter_map(Value::as_str)
                .map(|kind| self.render_type(kind, object, depth))
                .collect::<Vec<_>>()
                .join("|"),
            Some(Value::String(kind)) => self.render_type(kind, object, depth),
            _ => "JSON".to_owned(),
        }
    }

    fn render_type(&self, kind: &str, object: &Map<String, Value>, depth: usize) -> String {
        match kind {
            "string" => "String".to_owned(),
            "boolean" => "Boolean".to_owned(),
            "number" => "Float".to_owned(),
            "null" => "null".to_owned(),
            "integer" => {
                let minimum = object.get("minimum").and_then(Value::as_i64);
                let maximum = object.get("maximum").and_then(Value::as_i64);
                match (minimum, maximum) {
                    (Some(minimum), Some(maximum))
                        if minimum != i64::from(i32::MIN) || maximum != i64::from(i32::MAX) =>
                    {
                        format!("Int({minimum}..{maximum})")
                    }
                    _ => "Int".to_owned(),
                }
            }
            "array" => {
                let item = object
                    .get("items")
                    .map_or_else(|| "JSON".to_owned(), |items| self.render(items, depth + 1));
                format!("[{item}]")
            }
            "object" => {
                if object
                    .get("properties")
                    .and_then(Value::as_object)
                    .is_some_and(|properties| !properties.is_empty())
                {
                    let wrapped = Value::Object(object.clone());
                    format!("{{{}}}", self.render_fields_inline(&wrapped, depth))
                } else if let Some(values) = object.get("additionalProperties")
                    && values.is_object()
                {
                    format!("{{[key]: {}}}", self.render(values, depth + 1))
                } else {
                    "{}".to_owned()
                }
            }
            _ => "JSON".to_owned(),
        }
    }
}

fn required_names(object: &Map<String, Value>) -> BTreeSet<String> {
    object
        .get("required")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn insert_page(
    page: &mut Map<String, Value>,
    key: &str,
    items: Vec<Value>,
    offset: usize,
    limit: usize,
) {
    let total = items.len();
    let start = offset.min(total);
    let end = start.saturating_add(limit).min(total);
    page.insert("total".to_owned(), json!(total));
    page.insert("offset".to_owned(), json!(start));
    page.insert(key.to_owned(), Value::Array(items[start..end].to_vec()));
    page.insert(
        "nextOffset".to_owned(),
        if end < total { json!(end) } else { Value::Null },
    );
}

fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}.{name}")
    }
}

fn invalid(message: &str) -> AiError {
    AiError::InvalidConfiguration(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Value {
        let filter = json!({
            "type": "object",
            "properties": {
                "eq": {"anyOf": [{"type": "string", "maxLength": 256}, {"type": "null"}]},
                "in": {"anyOf": [{"type": "array", "items": {"type": "string"}}, {"type": "null"}]}
            },
            "required": [],
            "additionalProperties": false
        });
        json!({
            "type": "object",
            "properties": {
                "arguments": {
                    "type": "object",
                    "properties": {
                        "where": {"anyOf": [{
                            "type": "object",
                            "properties": {"title": filter, "status": filter},
                            "required": [],
                            "additionalProperties": false
                        }, {"type": "null"}]},
                        "status": {"enum": ["OPEN", "CLOSED"]}
                    },
                    "required": ["status"],
                    "additionalProperties": false
                },
                "selections": {
                    "type": "array",
                    "items": {"type": "string", "enum": ["id", "title", "owner.id", "owner.name"]}
                },
                "relationshipArguments": {
                    "type": "object",
                    "properties": {"owner": {
                        "type": "object",
                        "properties": {"active": {"type": "boolean"}},
                        "required": [],
                        "additionalProperties": false
                    }},
                    "required": [],
                    "additionalProperties": false
                },
                "relationshipMaximumItems": {
                    "type": "object",
                    "properties": {"owner": {"type": "integer", "minimum": 1, "maximum": 25}},
                    "required": [],
                    "additionalProperties": false
                },
                "maximumItems": {"type": "integer", "minimum": 1, "maximum": 100}
            },
            "required": ["arguments", "selections", "maximumItems"],
            "additionalProperties": false
        })
    }

    fn names(schema: &Value) -> BTreeMap<String, String> {
        let filter = &schema["properties"]["arguments"]["properties"]["where"]["anyOf"][0]["properties"]
            ["title"];
        let filter_where = &schema["properties"]["arguments"]["properties"]["where"]["anyOf"][0];
        BTreeMap::from([
            (
                AiSharedTypePlanSchema::type_name_key(filter),
                "StringFilterInput".to_owned(),
            ),
            (
                AiSharedTypePlanSchema::type_name_key(filter_where),
                "RecordWhereInput".to_owned(),
            ),
        ])
    }

    fn signature() -> AiCapabilitySignature {
        let schema = plan();
        let shared =
            AiSharedTypePlanSchema::compile_named(&schema, &names(&schema)).expect("shared");
        assert_eq!(shared.expand().expect("expanded"), schema);
        AiCapabilitySignature::compile("Records", shared).expect("signature")
    }

    #[test]
    fn signature_names_input_types_and_renders_the_selection_tree() {
        let signature = signature();
        assert_eq!(
            signature.signature(),
            r#"Records(where?: RecordWhereInput|null, status: "OPEN"|"CLOSED") -> select"#
        );
        let summary = signature.summary();
        assert_eq!(summary["select"], "id, owner{…}, title");
        assert_eq!(summary["maximumItems"]["type"], "Int(1..100)");
        assert_eq!(summary["maximumItems"]["required"], true);
        assert_eq!(
            summary["inputTypes"],
            json!(["RecordWhereInput", "StringFilterInput"])
        );
        assert_eq!(
            signature.type_names(),
            [
                "RecordWhereInput",
                "StringFilterInput",
                "select",
                "select.owner"
            ]
        );
    }

    #[test]
    fn describe_type_expands_one_type_in_pages() {
        let signature = signature();
        let page = signature
            .describe_type("RecordWhereInput", 0, 1)
            .expect("input type");
        assert_eq!(page["kind"], "input");
        assert_eq!(page["total"], 2);
        assert_eq!(page["fields"][0]["name"], "title");
        assert_eq!(page["fields"][0]["type"], "StringFilterInput");
        assert_eq!(page["nextOffset"], 1);
        let next = signature
            .describe_type("RecordWhereInput", 1, 1)
            .expect("second page");
        assert_eq!(next["fields"][0]["name"], "status");
        assert_eq!(next["nextOffset"], Value::Null);

        let filter = signature
            .describe_type("StringFilterInput", 0, 64)
            .expect("filter");
        assert_eq!(filter["fields"][0]["name"], "eq");
        assert_eq!(filter["fields"][0]["type"], "String|null");
        assert_eq!(filter["fields"][1]["type"], "[String]|null");

        let owner = signature.describe_type("select", 0, 64).expect("selection");
        assert_eq!(owner["kind"], "selection");
        assert_eq!(owner["fields"][1]["name"], "owner");
        assert_eq!(owner["fields"][1]["type"], "select.owner");
        assert_eq!(owner["fields"][1]["arguments"], "(active?: Boolean)");
        assert_eq!(owner["fields"][1]["maximumItems"], "Int(1..25)");
        let nested = signature
            .describe_type("select.owner", 0, 64)
            .expect("nested selection");
        assert_eq!(
            nested["fields"],
            json!([
                {"name": "id", "type": "scalar"},
                {"name": "name", "type": "scalar"}
            ])
        );
        assert!(signature.describe_type("select.missing", 0, 64).is_none());
        assert!(signature.describe_type("Unknown", 0, 64).is_none());
    }

    #[test]
    fn a_fixed_result_has_no_selection_types() {
        let schema = json!({
            "type": "object",
            "properties": {
                "arguments": {
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"],
                    "additionalProperties": false
                },
                "selections": {"type": "array", "maxItems": 0}
            },
            "required": ["arguments", "selections"],
            "additionalProperties": false
        });
        let signature = AiCapabilitySignature::compile(
            "lookup",
            AiSharedTypePlanSchema::compile_named(&schema, &BTreeMap::new()).expect("shared"),
        )
        .expect("signature");
        assert_eq!(signature.signature(), "lookup(query: String) -> fixed");
        assert!(signature.type_names().is_empty());
        assert!(signature.summary().get("select").is_none());
    }
}

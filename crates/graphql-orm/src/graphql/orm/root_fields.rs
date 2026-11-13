//! Compile-time uniqueness of composed GraphQL root fields.
//!
//! `schema_roots!` merges the generated root of every entity with the host's
//! handwritten roots. async-graphql serves a merged field from the first
//! member that declares it, so a handwritten field with the name of a
//! generated operation would silently replace it, or be replaced by it,
//! together with its validation and authorization. The names on both sides
//! are known when the schema is declared, so the collision is a compile error.

/// Root field names a handwritten `async-graphql` root declares.
///
/// `#[graphql_orm_custom_operations]` and `#[graphql_orm_root_fields]`
/// implement it from the resolvers they are applied to. `schema_roots!`
/// requires it of every handwritten root it composes.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is composed by `schema_roots!` but does not declare its root field names",
    label = "this root's field names are unknown to `schema_roots!`",
    note = "add `#[graphql_orm_root_fields]` directly before its `#[Object]` or `#[Subscription]` attribute, or describe it with `#[graphql_orm_custom_operations(kind = \"...\")]`",
    note = "`schema_roots!` rejects a handwritten root field that has the name of a generated operation, because a merged root would serve only one of them"
)]
pub trait GraphqlRootFieldNames {
    /// Public GraphQL names of the root fields this type contributes.
    const ROOT_FIELD_NAMES: &'static [&'static str];
}

/// The root fields one composed type contributes.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct RootFieldSource {
    /// Entity name for a generated root, type name for a handwritten one.
    pub owner: &'static str,
    /// Public GraphQL field names.
    pub fields: &'static [&'static str],
}

const fn same_name(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

const MESSAGE_CAPACITY: usize = 640;

struct Message {
    bytes: [u8; MESSAGE_CAPACITY],
    length: usize,
}

impl Message {
    const fn new() -> Self {
        Self {
            bytes: [0; MESSAGE_CAPACITY],
            length: 0,
        }
    }

    /// Append `text`, cut at the capacity on a character boundary.
    const fn push(mut self, text: &str) -> Self {
        let text = text.as_bytes();
        let mut index = 0;
        while index < text.len() && self.length < MESSAGE_CAPACITY {
            // Never start a multi-byte character that cannot be completed.
            let width = match text[index] {
                0x00..=0x7F => 1,
                0xC0..=0xDF => 2,
                0xE0..=0xEF => 3,
                _ => 4,
            };
            if self.length + width > MESSAGE_CAPACITY || index + width > text.len() {
                break;
            }
            let mut offset = 0;
            while offset < width {
                self.bytes[self.length] = text[index + offset];
                self.length += 1;
                offset += 1;
            }
            index += width;
        }
        self
    }
}

/// Panic with a built [`Message`]. A macro, so that the panic is raised in
/// the checking function itself and the compile error shows a single frame.
macro_rules! fail {
    ($message:expr) => {{
        let message = $message;
        let (text, _) = message.bytes.split_at(message.length);
        match core::str::from_utf8(text) {
            Ok(text) => panic!("{}", text),
            Err(_) => panic!("graphql-orm: colliding GraphQL root fields"),
        }
    }};
}

/// Fail compilation when two composed types contribute the same field to one
/// root: a handwritten field with the name of a generated operation, two
/// handwritten roots that declare the same field, or two entities whose
/// generated operations share a name (for example the same `plural`).
///
/// `root` is `Query`, `Mutation` or `Subscription`. Evaluated in a `const`
/// item that `schema_roots!` emits, so the panic is a compile error.
#[doc(hidden)]
pub const fn assert_unique_root_fields(
    root: &str,
    generated: &[RootFieldSource],
    handwritten: &[RootFieldSource],
) {
    let mut entity = 0;
    while entity < generated.len() {
        let mut operation = 0;
        while operation < generated[entity].fields.len() {
            let name = generated[entity].fields[operation];
            let mut sibling = operation + 1;
            while sibling < generated[entity].fields.len() {
                if same_name(name, generated[entity].fields[sibling]) {
                    fail!(Message::new()
                        .push("graphql-orm: entity `")
                        .push(generated[entity].owner)
                        .push("` generates the ")
                        .push(root)
                        .push(" root field `")
                        .push(name)
                        .push("` twice, so one of its operations would silently replace the other. This happens when two generated names coincide, for example a `plural` equal to the entity's name. Change the `plural`, or remove one operation with `query_shapes`/`mutation_shapes`."));
                }
                sibling += 1;
            }
            let mut other = entity + 1;
            while other < generated.len() {
                let mut other_operation = 0;
                while other_operation < generated[other].fields.len() {
                    if same_name(name, generated[other].fields[other_operation]) {
                        fail!(Message::new()
                            .push("graphql-orm: generated ")
                            .push(root)
                            .push(" root field `")
                            .push(name)
                            .push("` is produced by both entity `")
                            .push(generated[entity].owner)
                            .push("` and entity `")
                            .push(generated[other].owner)
                            .push("`. A merged root serves only one of them, so one entity's operation would silently replace the other's. Give one entity a different `plural` or GraphQL name, or remove the operation from one of them with `query_shapes`/`mutation_shapes`."));
                    }
                    other_operation += 1;
                }
                other += 1;
            }
            operation += 1;
        }
        entity += 1;
    }
    let mut custom = 0;
    while custom < handwritten.len() {
        let source = handwritten[custom];
        let mut field = 0;
        while field < source.fields.len() {
            let name = source.fields[field];
            let mut entity = 0;
            while entity < generated.len() {
                let mut operation = 0;
                while operation < generated[entity].fields.len() {
                    if same_name(name, generated[entity].fields[operation]) {
                        fail!(Message::new()
                            .push("graphql-orm: handwritten ")
                            .push(root)
                            .push(" root field `")
                            .push(name)
                            .push("` of `")
                            .push(source.owner)
                            .push("` collides with the generated operation `")
                            .push(name)
                            .push("` of entity `")
                            .push(generated[entity].owner)
                            .push("`. A merged root serves only one of them, so one would silently replace the other. Rename the handwritten field with `#[graphql(name = \"...\")]`, or remove the generated operation with the entity's `query_shapes`/`mutation_shapes` or `generated_mutations` in `schema_roots!`."));
                    }
                    operation += 1;
                }
                entity += 1;
            }
            let mut other = custom + 1;
            while other < handwritten.len() {
                let mut other_field = 0;
                while other_field < handwritten[other].fields.len() {
                    if same_name(name, handwritten[other].fields[other_field]) {
                        fail!(Message::new()
                            .push("graphql-orm: handwritten ")
                            .push(root)
                            .push(" root field `")
                            .push(name)
                            .push("` is declared by both `")
                            .push(source.owner)
                            .push("` and `")
                            .push(handwritten[other].owner)
                            .push("`. A merged root serves only one of them, so one would silently replace the other. Rename one with `#[graphql(name = \"...\")]`, or remove it."));
                    }
                    other_field += 1;
                }
                other += 1;
            }
            field += 1;
        }
        custom += 1;
    }
}

/// The fields `declared` omits from the root fields a type really exposes.
fn undeclared_root_fields(
    registry: &async_graphql::registry::Registry,
    type_name: &str,
    declared: &[&str],
) -> Vec<String> {
    let fields = match registry.types.get(type_name) {
        Some(async_graphql::registry::MetaType::Object { fields, .. }) => fields,
        _ => return Vec::new(),
    };
    fields
        .keys()
        .filter(|field| !declared.contains(&field.as_str()))
        .cloned()
        .collect()
}

fn refuse_undeclared_root_fields(owner: &str, undeclared: Vec<String>) {
    assert!(
        undeclared.is_empty(),
        "graphql-orm: `{owner}` exposes root field(s) `{}` that its `GraphqlRootFieldNames` \
         implementation does not declare, so `schema_roots!` could not check them against \
         generated operations. Declare every root field: use `#[graphql_orm_root_fields]` on \
         an `#[Object]` or `#[Subscription]` impl, or list the fields of every member in a \
         manual implementation for a merged root.",
        undeclared.join("`, `"),
    );
}

/// Check, when a schema is built, that a handwritten Query or Mutation root
/// declares every root field it exposes.
///
/// The compile-time collision check trusts [`GraphqlRootFieldNames`]. A
/// manual implementation (the only route for a `MergedObject` root) that
/// leaves a field out would hide a collision from it, so the generated schema
/// builders compare the declaration with the type's real fields.
///
/// # Panics
///
/// Panics when the type exposes a root field its declaration omits.
#[doc(hidden)]
pub fn assert_declared_object_root_fields<T>(owner: &str, declared: &[&str])
where
    T: async_graphql::OutputType,
{
    let mut registry = async_graphql::registry::Registry::default();
    T::create_type_info(&mut registry);
    refuse_undeclared_root_fields(
        owner,
        undeclared_root_fields(&registry, &T::type_name(), declared),
    );
}

/// [`assert_declared_object_root_fields`] for a handwritten Subscription root.
///
/// # Panics
///
/// Panics when the type exposes a root field its declaration omits.
#[doc(hidden)]
pub fn assert_declared_subscription_root_fields<T>(owner: &str, declared: &[&str])
where
    T: async_graphql::SubscriptionType,
{
    let mut registry = async_graphql::registry::Registry::default();
    T::create_type_info(&mut registry);
    refuse_undeclared_root_fields(
        owner,
        undeclared_root_fields(&registry, &T::type_name(), declared),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENERATED: &[RootFieldSource] = &[RootFieldSource {
        owner: "Item",
        fields: &["item", "items"],
    }];

    // Distinct names compile; the colliding cases are UI tests.
    const _: () = assert_unique_root_fields(
        "Query",
        GENERATED,
        &[
            RootFieldSource {
                owner: "StatusQueries",
                fields: &["status", "Items"],
            },
            RootFieldSource {
                owner: "ReportQueries",
                fields: &["report"],
            },
        ],
    );

    #[test]
    fn a_message_never_splits_a_character_or_exceeds_its_capacity() {
        let message = Message::new().push(&"é".repeat(MESSAGE_CAPACITY));
        assert_eq!(message.length, MESSAGE_CAPACITY);
        assert!(core::str::from_utf8(&message.bytes[..message.length]).is_ok());
        let message = Message::new().push("a").push(&"é".repeat(MESSAGE_CAPACITY));
        assert_eq!(message.length, MESSAGE_CAPACITY - 1);
        assert!(core::str::from_utf8(&message.bytes[..message.length]).is_ok());
    }

    #[test]
    #[should_panic(
        expected = "handwritten Mutation root field `deleteItem` of `ItemActions` collides with the generated operation `deleteItem` of entity `Item`"
    )]
    fn a_generated_collision_names_both_sides() {
        assert_unique_root_fields(
            "Mutation",
            &[RootFieldSource {
                owner: "Item",
                fields: &["createItem", "deleteItem"],
            }],
            &[RootFieldSource {
                owner: "ItemActions",
                fields: &["deleteItem"],
            }],
        );
    }

    #[test]
    #[should_panic(
        expected = "generated Query root field `items` is produced by both entity `Item` and entity `StockItem`"
    )]
    fn two_entities_cannot_generate_the_same_root_field() {
        assert_unique_root_fields(
            "Query",
            &[
                GENERATED[0],
                RootFieldSource {
                    owner: "StockItem",
                    fields: &["stockItem", "items"],
                },
            ],
            &[],
        );
    }

    #[test]
    #[should_panic(expected = "is declared by both `StatusQueries` and `ReportQueries`")]
    fn two_handwritten_roots_cannot_share_a_field() {
        assert_unique_root_fields(
            "Query",
            GENERATED,
            &[
                RootFieldSource {
                    owner: "StatusQueries",
                    fields: &["status"],
                },
                RootFieldSource {
                    owner: "ReportQueries",
                    fields: &["status"],
                },
            ],
        );
    }
}

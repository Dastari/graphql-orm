//! Recover source declaration order after the pinned composer sorts members.
//! Only surviving composed members are reordered; composition still owns membership.
use std::collections::BTreeMap;

use hive_router::graphql_tools::static_graphql::schema::{
    Definition, Document, Field, TypeDefinition, TypeExtension,
};

#[derive(Default)]
pub(super) struct DeclarationOrder(BTreeMap<String, Vec<String>>);

impl DeclarationOrder {
    pub(super) fn record(&mut self, document: &cynic_parser::TypeSystemDocument) {
        use cynic_parser::type_system::{Definition as D, TypeDefinition as T};
        for definition in document.definitions() {
            let ty = match definition {
                D::Type(t) | D::TypeExtension(t) => t,
                D::Directive(d) => {
                    self.names(&format!("@{}", d.name()), d.arguments().map(|a| a.name()));
                    continue;
                }
                _ => continue,
            };
            match ty {
                T::Object(t) => self.record_fields(t.name(), t.fields()),
                T::Interface(t) => self.record_fields(t.name(), t.fields()),
                T::InputObject(t) => self.names(t.name(), t.fields().map(|f| f.name())),
                T::Enum(t) => self.names(t.name(), t.values().map(|v| v.value())),
                _ => {}
            }
        }
    }

    fn names<'a>(&mut self, coordinate: &str, names: impl Iterator<Item = &'a str>) {
        let order = self.0.entry(coordinate.to_owned()).or_default();
        for name in names {
            if !order.iter().any(|existing| existing == name) {
                order.push(name.to_owned());
            }
        }
    }

    fn record_fields<'a>(
        &mut self,
        parent: &str,
        fields: impl Iterator<Item = cynic_parser::type_system::FieldDefinition<'a>> + Clone,
    ) {
        self.names(parent, fields.clone().map(|f| f.name()));
        for field in fields {
            self.names(
                &format!("{parent}.{}", field.name()),
                field.arguments().map(|a| a.name()),
            );
        }
    }

    fn members<T>(&self, coordinate: &str, members: &mut [T], name: impl Fn(&T) -> &str) {
        if let Some(order) = self.0.get(coordinate) {
            // Stable sorting retains the composer's order for synthetic members.
            members.sort_by_key(|member| {
                order
                    .iter()
                    .position(|n| n == name(member))
                    .unwrap_or(usize::MAX)
            });
        }
    }

    fn fields(&mut self, parent: &str, fields: &mut [Field]) {
        self.members(parent, fields, |f| &f.name);
        for field in fields {
            self.members(
                &format!("{parent}.{}", field.name),
                &mut field.arguments,
                |a| &a.name,
            );
        }
    }

    pub(super) fn restore(&mut self, document: &mut Document) {
        for definition in &mut document.definitions {
            // Definitions and extensions have distinct AST types but identical
            // member ordering rules. Never rewrite SDL text or federation metadata.
            macro_rules! visit_type {
                ($kind:ident, $value:expr) => {
                    match $value {
                        $kind::Object(t) => self.fields(&t.name, &mut t.fields),
                        $kind::Interface(t) => self.fields(&t.name, &mut t.fields),
                        $kind::InputObject(t) => {
                            self.members(&t.name, &mut t.fields, |f| &f.name);
                        }
                        $kind::Enum(t) => {
                            self.members(&t.name, &mut t.values, |v| &v.name);
                        }
                        _ => {}
                    }
                };
            }
            match definition {
                Definition::TypeDefinition(t) => visit_type!(TypeDefinition, t),
                Definition::TypeExtension(t) => visit_type!(TypeExtension, t),
                Definition::DirectiveDefinition(d) => {
                    self.members(&format!("@{}", d.name), &mut d.arguments, |a| &a.name);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_router::query_planner::utils::parsing::safe_parse_schema;

    #[test]
    fn shared_members_extensions_and_synthetic_members_keep_deterministic_order() {
        let mut order = DeclarationOrder::default();
        for source in [
            "extend schema @link(url: \"https://specs.apollo.dev/federation/v2.3\") type Query { z(b: Int, a: Int): Int } input Key { site: ID } extend input Key { name: ID }",
            "type Query { a: Int z(a: Int, b: Int): Int } input Key { name: ID site: ID extra: ID }",
        ] {
            order.record(&cynic_parser::parse_type_system_document(source).unwrap());
        }
        let mut composed = safe_parse_schema(
            "type Query { a: Int synthetic: Int z(a: Int, b: Int): Int } input Key { extra: ID name: ID site: ID }",
        ).unwrap();
        order.restore(&mut composed);
        let expected = safe_parse_schema(
            "type Query { z(b: Int, a: Int): Int a: Int synthetic: Int } input Key { site: ID name: ID extra: ID }",
        ).unwrap();
        assert_eq!(composed.to_string(), expected.to_string());
    }
}

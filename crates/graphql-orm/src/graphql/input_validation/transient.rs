use async_graphql::{
    Name, Pos, Positioned, ServerError, Value,
    parser::types::{Directive, ExecutableDocument, Selection, SelectionSet},
    registry::{MetaType, Registry},
};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;

use super::operation_root;
use crate::graphql::orm::TRANSIENT_INPUT_INVALID;

/// async-graphql 7.2.1 reports argument value errors at the argument name
/// and default value errors at the variable definition. Match those source
/// locations against the schema and document, independent of error wording.
pub(super) fn redact_errors(
    registry: &Registry,
    document: &ExecutableDocument,
    errors: &mut [ServerError],
) {
    let error_locations: HashSet<_> = errors
        .iter()
        .filter_map(|error| match error.locations.as_slice() {
            [position] => Some(*position),
            _ => None,
        })
        .collect();
    if error_locations.is_empty() {
        return;
    }

    // Strict validation checks every operation and fragment, including those
    // outside the requested operation. A default belongs to a particular
    // operation, while a fragment can use that operation's variables.
    let mut locations = RedactionLocations::default();
    let fragments: HashMap<_, _> = document
        .fragments
        .iter()
        .map(|(name, fragment)| {
            let fragment = &fragment.node;
            let uses = TransientUses::collect(
                registry,
                registry
                    .types
                    .get(fragment.type_condition.node.on.node.as_str()),
                &fragment.selection_set.node,
                &fragment.directives,
                &mut locations,
            );
            (name, uses)
        })
        .collect();
    for (_, operation) in document.operations.iter() {
        let operation = &operation.node;
        let root = operation_root(registry, operation).and_then(|name| registry.types.get(name));
        let mut uses = TransientUses::collect(
            registry,
            root,
            &operation.selection_set.node,
            &operation.directives,
            &mut locations,
        );
        let defaults: Vec<_> = operation
            .variable_definitions
            .iter()
            .filter(|definition| {
                definition.node.default_value.is_some() && error_locations.contains(&definition.pos)
            })
            .collect();
        if defaults.is_empty() {
            continue;
        }
        let mut seen = HashSet::new();
        while let Some(name) = uses.spreads.pop() {
            if seen.insert(name)
                && let Some(fragment) = fragments.get(name)
            {
                uses.variables.extend(fragment.variables.iter().cloned());
                uses.spreads.extend(&fragment.spreads);
            }
        }
        locations.redacted.extend(
            defaults
                .into_iter()
                .filter(|definition| uses.variables.contains(&definition.node.name.node))
                .map(|definition| definition.pos),
        );
    }

    // With no selected operation, validation can substitute the supplied value
    // at every use of the name across the document.
    for (position, variables) in locations.ordinary_arguments {
        if variables
            .iter()
            .any(|name| locations.transient_variables.contains(name))
        {
            locations.redacted.insert(position);
        }
    }
    for error in errors {
        if matches!(error.locations.as_slice(), [position] if locations.redacted.contains(position))
        {
            error.message = TRANSIENT_INPUT_INVALID.to_owned();
        }
    }
}

#[derive(Default)]
struct RedactionLocations {
    redacted: HashSet<Pos>,
    transient_variables: HashSet<Name>,
    ordinary_arguments: Vec<(Pos, Vec<Name>)>,
}

#[derive(Default)]
struct TransientUses<'a> {
    variables: HashSet<Name>,
    spreads: Vec<&'a Name>,
}

impl<'a> TransientUses<'a> {
    fn collect(
        registry: &Registry,
        root: Option<&MetaType>,
        selection_set: &'a SelectionSet,
        directives: &'a [Positioned<Directive>],
        locations: &mut RedactionLocations,
    ) -> Self {
        let mut uses = Self::default();
        let directive_arguments = |directives: &'a [Positioned<Directive>]| {
            directives
                .iter()
                .flat_map(|directive| &directive.node.arguments)
                .map(|argument| (argument, false))
        };
        let mut arguments: Vec<_> = directive_arguments(directives).collect();
        let mut pending = vec![(root, selection_set)];
        while let Some((parent, set)) = pending.pop() {
            for selection in &set.items {
                arguments.extend(directive_arguments(selection.node.directives()));
                match &selection.node {
                    Selection::Field(field) => {
                        let field = &field.node;
                        let definition = parent.and_then(|ty| ty.field_by_name(&field.name.node));
                        arguments.extend(field.arguments.iter().map(|argument| {
                            let (name, _) = argument;
                            let marked = definition
                                .and_then(|field| field.args.get(name.node.as_str()))
                                .is_some_and(|arg| {
                                    arg.directive_invocations
                                        .iter()
                                        .any(|d| d.name == "transientInput")
                                });
                            (argument, marked)
                        }));
                        pending.push((
                            definition.and_then(|field| registry.concrete_type_by_name(&field.ty)),
                            &field.selection_set.node,
                        ));
                    }
                    Selection::InlineFragment(fragment) => {
                        let fragment = &fragment.node;
                        let parent = match &fragment.type_condition {
                            Some(condition) => registry.types.get(condition.node.on.node.as_str()),
                            None => parent,
                        };
                        pending.push((parent, &fragment.selection_set.node));
                    }
                    Selection::FragmentSpread(spread) => {
                        uses.spreads.push(&spread.node.fragment_name.node);
                    }
                }
            }
        }
        for ((name, value), marked) in arguments {
            let mut variables = Vec::new();
            let _ = value.node.clone().into_const_with(|name| {
                variables.push(name);
                Ok::<_, Infallible>(Value::Null)
            });
            if marked {
                locations.redacted.insert(name.pos);
                locations
                    .transient_variables
                    .extend(variables.iter().cloned());
                uses.variables.extend(variables);
            } else if !variables.is_empty() {
                locations.ordinary_arguments.push((name.pos, variables));
            }
        }
        uses
    }
}

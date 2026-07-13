# Federation Operation Roots

`schema_roots!` keeps its public Rust API:

```rust
type ProviderSchema = async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>;
```

The corresponding nonempty GraphQL object names are `Query`, `Mutation`, and `Subscription`.
Async-graphql's federation exporter omits an explicit `schema { ... }` definition, so these
conventional names provide the standards-defined implicit operation mapping. Generated fields are
therefore direct fields of the query operation root rather than fields on an unreachable
`QueryRoot` child type.

Export federation SDL normally:

```rust
let sdl = schema.sdl_with_options(
    async_graphql::SDLExportOptions::new().federation(),
);
```

Async-graphql excludes subscriptions from federation SDL by default. When a federation deployment
intentionally exposes subscriptions, opt in while building the schema:

```rust
let schema = schema_builder(database)
    .enable_subscription_in_federation()
    .finish();
```

`EmptyMutation` and `EmptySubscription` remain absent from the exported SDL. They never create a
dangling operation mapping or a fieldless placeholder object.

## Composition Acceptance

The repository tests parse the actual macro-generated federation SDL and resolve explicit or
conventional operation roots independently of async-graphql. They assert that every declared root
exists and that generated provider fields, including an exact `NinjaDevices` fixture, are direct
members of `Query`.

For a connected Cosmo graph, validate regenerated provider SDL without publishing it:

```bash
npx wgc subgraph check <subgraph-name> --schema ./provider.graphql -n <namespace>
```

For isolated local composition, place the regenerated subgraphs in a WGC router input file and
build the execution configuration:

```bash
npx wgc router compose -i ./graph.yaml -o ./router.json
```

Review the resulting query root nodes before promotion. Do not post-process the generated SDL to
rename or reattach operation types.

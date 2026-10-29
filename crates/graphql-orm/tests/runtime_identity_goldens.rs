//! Identity goldens for a runtime schema that declares no automatic timestamps and no
//! actor stamps.
//!
//! The expected values were captured from graphql-orm 0.71.0
//! (`01c966a9d86c1726a95018fa33af03dc67ef0aed`), before `RuntimeField::auto_timestamp`
//! existed, by running this file unchanged. Equal values prove that adding that member,
//! and later `RuntimeField::auto_actor` (0.81.0), left serialized documents, canonical
//! bytes, fingerprints and the physical target of schemas without them byte-identical. The fixture is JSON, so the file compiles against
//! both releases. Run with `GRAPHQL_ORM_PRINT_GOLDENS=1` to print a fresh report.
#![cfg(feature = "sqlite")]

use graphql_orm::graphql::orm::*;

fn fixture() -> serde_json::Value {
    let field = |id: &str, api: &str, kind: &str, nullable: bool, default: serde_json::Value| {
        serde_json::json!({
            "id": id, "api_name": api, "physical_column": api, "value_kind": kind,
            "nullable": nullable, "unique": false, "filterable": true, "sortable": true,
            "generated": false, "default": default
        })
    };
    let null = serde_json::Value::Null;
    serde_json::json!({
        "collections": [
            {
                "id": "idg_parent", "api_type_name": "Parent", "api_plural_name": "Parents",
                "physical_table": "idg_parents", "primary_key": ["idg_parent.id"],
                "append_only": false,
                "fields": [field("idg_parent.id", "id", "integer", false, null.clone())],
                "relations": [], "indexes": [], "composite_unique": [], "default_order": []
            },
            {
                "id": "idg_tag", "api_type_name": "Tag", "api_plural_name": "Tags",
                "physical_table": "idg_tags", "primary_key": ["idg_tag.id"],
                "append_only": false,
                "fields": [field("idg_tag.id", "id", "integer", false, null.clone())],
                "relations": [], "indexes": [], "composite_unique": [], "default_order": []
            },
            {
                "id": "idg_note", "api_type_name": "Note", "api_plural_name": "Notes",
                "physical_table": "idg_notes", "primary_key": ["idg_note.id"],
                "append_only": false,
                "description": "A note",
                "fields": [
                    field("idg_note.id", "id", "integer", false, null.clone()),
                    field("idg_note.title", "title", "string", false, serde_json::json!({"literal": "untitled"})),
                    field("idg_note.parent_id", "parent_id", "integer", true, null.clone()),
                    field("idg_note.created", "created", "date_time", false, serde_json::json!("current_timestamp")),
                    field("idg_note.edited", "edited", "date_time", true, null),
                ],
                "relations": [
                    {
                        "id": "idg_note.parent", "api_name": "parent", "target": "idg_parent",
                        "key_pairs": [{"source": "idg_note.parent_id", "target": "idg_parent.id"}],
                        "cardinality": "one", "enforce_foreign_key": true, "on_delete": "SetNull"
                    },
                    {
                        "id": "idg_note.tags", "api_name": "tags", "target": "idg_tag",
                        "key_pairs": [], "cardinality": "many", "enforce_foreign_key": false,
                        "on_delete": null,
                        "join": {
                            "physical_table": "idg_note_tags",
                            "source": [{"field": "idg_note.id", "column": "note_id"}],
                            "target": [{"field": "idg_tag.id", "column": "tag_id"}]
                        }
                    }
                ],
                "indexes": [], "composite_unique": [],
                "default_order": [{"field": "idg_note.created", "direction": "desc"}],
                "label_fields": ["idg_note.title"]
            }
        ]
    })
}

fn report() -> Vec<(&'static str, String)> {
    let schema: RuntimeSchema = serde_json::from_value(fixture()).unwrap();
    let schema = schema.validate().unwrap();
    let target = schema
        .physical_schema::<SqliteBackend>(RuntimeMigrationLimits::default())
        .unwrap();
    vec![
        (
            "serialized",
            serde_json::to_string(schema.schema()).unwrap(),
        ),
        (
            "canonical",
            String::from_utf8(schema.canonical_bytes().to_vec()).unwrap(),
        ),
        ("fingerprint", schema.fingerprint().to_string()),
        ("structural", schema.structural_fingerprint().to_string()),
        ("format", schema.format_fingerprint().to_string()),
        ("physical_sqlite", target.stable_hash().to_string()),
    ]
}

#[test]
fn schemas_without_auto_timestamps_keep_their_0_71_0_identity() {
    let report = report();
    if std::env::var_os("GRAPHQL_ORM_PRINT_GOLDENS").is_some() {
        for (name, value) in &report {
            println!("GOLDEN {name}={}", serde_json::to_string(value).unwrap());
        }
        return;
    }
    let golden: [(&str, &str); 6] = GOLDEN;
    for ((name, actual), (expected_name, expected)) in report.iter().zip(golden) {
        assert_eq!(*name, expected_name);
        assert_eq!(actual, expected, "golden {name}");
    }
}

const GOLDEN: [(&str, &str); 6] = [
    (
        "serialized",
        r#"{"format_version":1,"collections":[{"id":"idg_parent","api_type_name":"Parent","api_plural_name":"Parents","physical_table":"idg_parents","primary_key":["idg_parent.id"],"append_only":false,"retention_purge":false,"fields":[{"id":"idg_parent.id","api_name":"id","physical_column":"id","value_kind":"integer","nullable":false,"unique":false,"filterable":true,"sortable":true,"generated":false,"default":null}],"relations":[],"indexes":[],"composite_unique":[],"default_order":[]},{"id":"idg_tag","api_type_name":"Tag","api_plural_name":"Tags","physical_table":"idg_tags","primary_key":["idg_tag.id"],"append_only":false,"retention_purge":false,"fields":[{"id":"idg_tag.id","api_name":"id","physical_column":"id","value_kind":"integer","nullable":false,"unique":false,"filterable":true,"sortable":true,"generated":false,"default":null}],"relations":[],"indexes":[],"composite_unique":[],"default_order":[]},{"id":"idg_note","api_type_name":"Note","api_plural_name":"Notes","physical_table":"idg_notes","primary_key":["idg_note.id"],"append_only":false,"retention_purge":false,"fields":[{"id":"idg_note.id","api_name":"id","physical_column":"id","value_kind":"integer","nullable":false,"unique":false,"filterable":true,"sortable":true,"generated":false,"default":null},{"id":"idg_note.title","api_name":"title","physical_column":"title","value_kind":"string","nullable":false,"unique":false,"filterable":true,"sortable":true,"generated":false,"default":{"literal":"untitled"}},{"id":"idg_note.parent_id","api_name":"parent_id","physical_column":"parent_id","value_kind":"integer","nullable":true,"unique":false,"filterable":true,"sortable":true,"generated":false,"default":null},{"id":"idg_note.created","api_name":"created","physical_column":"created","value_kind":"date_time","nullable":false,"unique":false,"filterable":true,"sortable":true,"generated":false,"default":"current_timestamp"},{"id":"idg_note.edited","api_name":"edited","physical_column":"edited","value_kind":"date_time","nullable":true,"unique":false,"filterable":true,"sortable":true,"generated":false,"default":null}],"relations":[{"id":"idg_note.parent","api_name":"parent","target":"idg_parent","key_pairs":[{"source":"idg_note.parent_id","target":"idg_parent.id"}],"cardinality":"one","enforce_foreign_key":true,"on_delete":"SetNull"},{"id":"idg_note.tags","api_name":"tags","target":"idg_tag","key_pairs":[],"cardinality":"many","enforce_foreign_key":false,"on_delete":null,"join":{"physical_table":"idg_note_tags","source":[{"field":"idg_note.id","column":"note_id"}],"target":[{"field":"idg_tag.id","column":"tag_id"}]}}],"indexes":[],"composite_unique":[],"default_order":[{"field":"idg_note.created","direction":"desc"}],"description":"A note","label_fields":["idg_note.title"]}]}"#,
    ),
    (
        "canonical",
        r#"runtime-schema v1
collection|id=idg_note|type=Note|plural=Notes|table=idg_notes|append_only=false|description="A note"|label=["idg_note.title"]
  primary_key|idg_note.id
  field|id=idg_note.created|api=created|column=created|kind=datetime|nullable=false|unique=false|filterable=true|sortable=true|generated=false|default=current_timestamp
  field|id=idg_note.edited|api=edited|column=edited|kind=datetime|nullable=true|unique=false|filterable=true|sortable=true|generated=false|default=none
  field|id=idg_note.id|api=id|column=id|kind=integer|nullable=false|unique=false|filterable=true|sortable=true|generated=false|default=none
  field|id=idg_note.parent_id|api=parent_id|column=parent_id|kind=integer|nullable=true|unique=false|filterable=true|sortable=true|generated=false|default=none
  field|id=idg_note.title|api=title|column=title|kind=string|nullable=false|unique=false|filterable=true|sortable=true|generated=false|default=literal:"untitled"
  relation|id=idg_note.parent|api=parent|target=idg_parent|keys=idg_note.parent_id=idg_parent.id|cardinality=one|fk=true|on_delete=set null
  relation|id=idg_note.tags|api=tags|target=idg_tag|keys=|cardinality=many|fk=false|on_delete=none|join=idg_note_tags|join_source=idg_note.id=note_id|join_target=idg_tag.id=tag_id
  default_order|idg_note.created desc
collection|id=idg_parent|type=Parent|plural=Parents|table=idg_parents|append_only=false
  primary_key|idg_parent.id
  field|id=idg_parent.id|api=id|column=id|kind=integer|nullable=false|unique=false|filterable=true|sortable=true|generated=false|default=none
collection|id=idg_tag|type=Tag|plural=Tags|table=idg_tags|append_only=false
  primary_key|idg_tag.id
  field|id=idg_tag.id|api=id|column=id|kind=integer|nullable=false|unique=false|filterable=true|sortable=true|generated=false|default=none
"#,
    ),
    ("fingerprint", r#"5601d911f8c578db"#),
    ("structural", r#"a4365362393dd160"#),
    ("format", r#"3dbe2f9677de9539"#),
    ("physical_sqlite", r#"1f2879b4df454abf"#),
];

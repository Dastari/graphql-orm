//! Private, complete, non-normalizing wire adapters. No public serde bypass.
use super::*;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

pub(super) trait Decode {
    type Output;
    fn finish(self) -> Self::Output;
}
macro_rules! scalar { ($($t:ty),*) => {$ (
impl Decode for $t { type Output=Self; fn finish(self)->Self {self} }
impl Wire for $t {
 fn emit<S:Serializer>(&self,s:S)->Result<S::Ok,S::Error>{self.serialize(s)}
 fn inventory(&self,i:&mut Inventory)->R<()> {i.scalar()}
}
)*}; }
scalar!(bool, usize, i32, u16);
impl Decode for String {
    type Output = Self;
    fn finish(self) -> Self {
        self
    }
}
impl<T: Decode> Decode for Vec<T> {
    type Output = Vec<T::Output>;
    fn finish(self) -> Self::Output {
        self.into_iter().map(Decode::finish).collect()
    }
}
impl<T: Decode> Decode for Option<T> {
    type Output = Option<T::Output>;
    fn finish(self) -> Self::Output {
        self.map(Decode::finish)
    }
}
pub(super) trait Wire {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error>;
    fn inventory(&self, i: &mut Inventory) -> R<()>;
}
pub(super) struct Enc<'a, T: ?Sized>(pub &'a T);
impl<T: Wire + ?Sized> Serialize for Enc<'_, T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.emit(s)
    }
}
impl Wire for String {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self)
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.string(self)
    }
}
impl<T: Wire> Wire for Vec<T> {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(Some(self.len()))?;
        for x in self {
            seq.serialize_element(&Enc(x))?;
        }
        seq.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        for x in self {
            x.inventory(i)?;
        }
        Ok(())
    }
}
impl<T: Wire> Wire for Option<T> {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Some(x) => s.serialize_some(&Enc(x)),
            None => s.serialize_none(),
        }
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        if let Some(x) = self {
            x.inventory(i)?;
        }
        Ok(())
    }
}
fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum SpatialKindWire {
    Geometry,
}
impl Decode for SpatialKindWire {
    type Output = SpatialKind;
    fn finish(self) -> SpatialKind {
        match self {
            Self::Geometry => SpatialKind::Geometry,
        }
    }
}
impl Wire for SpatialKind {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::Geometry => "geometry",
        })
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum GeometryWire {
    Geometry,
    Point,
    LineString,
    Polygon,
    MultiPoint,
    MultiLineString,
    MultiPolygon,
    GeometryCollection,
}
impl Decode for GeometryWire {
    type Output = SpatialGeometryType;
    fn finish(self) -> SpatialGeometryType {
        match self {
            Self::Geometry => SpatialGeometryType::Geometry,
            Self::Point => SpatialGeometryType::Point,
            Self::LineString => SpatialGeometryType::LineString,
            Self::Polygon => SpatialGeometryType::Polygon,
            Self::MultiPoint => SpatialGeometryType::MultiPoint,
            Self::MultiLineString => SpatialGeometryType::MultiLineString,
            Self::MultiPolygon => SpatialGeometryType::MultiPolygon,
            Self::GeometryCollection => SpatialGeometryType::GeometryCollection,
        }
    }
}
impl Wire for SpatialGeometryType {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::Geometry => "geometry",
            Self::Point => "point",
            Self::LineString => "lineString",
            Self::Polygon => "polygon",
            Self::MultiPoint => "multiPoint",
            Self::MultiLineString => "multiLineString",
            Self::MultiPolygon => "multiPolygon",
            Self::GeometryCollection => "geometryCollection",
        })
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum DirectionWire {
    Asc,
    Desc,
}
impl Decode for DirectionWire {
    type Output = IndexDirection;
    fn finish(self) -> IndexDirection {
        match self {
            Self::Asc => IndexDirection::Asc,
            Self::Desc => IndexDirection::Desc,
        }
    }
}
impl Wire for IndexDirection {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        })
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum MethodWire {
    Default,
    Gist,
}
impl Decode for MethodWire {
    type Output = IndexMethod;
    fn finish(self) -> IndexMethod {
        match self {
            Self::Default => IndexMethod::Default,
            Self::Gist => IndexMethod::Gist,
        }
    }
}
impl Wire for IndexMethod {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::Default => "default",
            Self::Gist => "gist",
        })
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum DeleteWire {
    Restrict,
    Cascade,
    SetNull,
}
impl Decode for DeleteWire {
    type Output = DeletePolicy;
    fn finish(self) -> DeletePolicy {
        match self {
            Self::Restrict => DeletePolicy::Restrict,
            Self::Cascade => DeletePolicy::Cascade,
            Self::SetNull => DeletePolicy::SetNull,
        }
    }
}
impl Wire for DeletePolicy {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::Restrict => "restrict",
            Self::Cascade => "cascade",
            Self::SetNull => "setNull",
        })
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()
    }
}
#[derive(Deserialize)]
pub(super) enum WeightWire {
    A,
    B,
    C,
    D,
}
impl Decode for WeightWire {
    type Output = SearchWeight;
    fn finish(self) -> SearchWeight {
        match self {
            Self::A => SearchWeight::A,
            Self::B => SearchWeight::B,
            Self::C => SearchWeight::C,
            Self::D => SearchWeight::D,
        }
    }
}
impl Wire for SearchWeight {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
        })
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum StrategyWire {
    PostgresTsvector,
    SqliteFts5,
    FallbackTable,
    MysqlFullText,
    MssqlFullText,
}
impl Decode for StrategyWire {
    type Output = SearchIndexStrategy;
    fn finish(self) -> SearchIndexStrategy {
        match self {
            Self::PostgresTsvector => SearchIndexStrategy::PostgresTsvector,
            Self::SqliteFts5 => SearchIndexStrategy::SqliteFts5,
            Self::FallbackTable => SearchIndexStrategy::FallbackTable,
            Self::MysqlFullText => SearchIndexStrategy::MysqlFullText,
            Self::MssqlFullText => SearchIndexStrategy::MssqlFullText,
        }
    }
}
impl Wire for SearchIndexStrategy {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::PostgresTsvector => "postgresTsvector",
            Self::SqliteFts5 => "sqliteFts5",
            Self::FallbackTable => "fallbackTable",
            Self::MysqlFullText => "mysqlFullText",
            Self::MssqlFullText => "mssqlFullText",
        })
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum RiskWire {
    Additive,
    Compatible,
    Risky,
    Destructive,
}
impl Decode for RiskWire {
    type Output = MigrationRisk;
    fn finish(self) -> MigrationRisk {
        match self {
            Self::Additive => MigrationRisk::Additive,
            Self::Compatible => MigrationRisk::Compatible,
            Self::Risky => MigrationRisk::Risky,
            Self::Destructive => MigrationRisk::Destructive,
        }
    }
}
impl Wire for MigrationRisk {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::Additive => "additive",
            Self::Compatible => "compatible",
            Self::Risky => "risky",
            Self::Destructive => "destructive",
        })
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Limits {
    max_collections: usize,
    max_fields: usize,
    max_indexes: usize,
    max_relations: usize,
    max_key_members: usize,
    max_default_bytes: usize,
    max_target_bytes: usize,
    max_plan_steps: usize,
    max_plan_statements: usize,
    max_plan_bytes: usize,
}
impl Decode for Limits {
    type Output = RuntimeMigrationLimits;
    fn finish(self) -> RuntimeMigrationLimits {
        RuntimeMigrationLimits {
            max_collections: self.max_collections.finish(),
            max_fields: self.max_fields.finish(),
            max_indexes: self.max_indexes.finish(),
            max_relations: self.max_relations.finish(),
            max_key_members: self.max_key_members.finish(),
            max_default_bytes: self.max_default_bytes.finish(),
            max_target_bytes: self.max_target_bytes.finish(),
            max_plan_steps: self.max_plan_steps.finish(),
            max_plan_statements: self.max_plan_statements.finish(),
            max_plan_bytes: self.max_plan_bytes.finish(),
        }
    }
}
impl Wire for RuntimeMigrationLimits {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("Limits", 10)?;
        o.serialize_field("maxCollections", &Enc(&self.max_collections))?;
        o.serialize_field("maxFields", &Enc(&self.max_fields))?;
        o.serialize_field("maxIndexes", &Enc(&self.max_indexes))?;
        o.serialize_field("maxRelations", &Enc(&self.max_relations))?;
        o.serialize_field("maxKeyMembers", &Enc(&self.max_key_members))?;
        o.serialize_field("maxDefaultBytes", &Enc(&self.max_default_bytes))?;
        o.serialize_field("maxTargetBytes", &Enc(&self.max_target_bytes))?;
        o.serialize_field("maxPlanSteps", &Enc(&self.max_plan_steps))?;
        o.serialize_field("maxPlanStatements", &Enc(&self.max_plan_statements))?;
        o.serialize_field("maxPlanBytes", &Enc(&self.max_plan_bytes))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("maxCollections")?;
        self.max_collections.inventory(i)?;
        i.member("maxFields")?;
        self.max_fields.inventory(i)?;
        i.member("maxIndexes")?;
        self.max_indexes.inventory(i)?;
        i.member("maxRelations")?;
        self.max_relations.inventory(i)?;
        i.member("maxKeyMembers")?;
        self.max_key_members.inventory(i)?;
        i.member("maxDefaultBytes")?;
        self.max_default_bytes.inventory(i)?;
        i.member("maxTargetBytes")?;
        self.max_target_bytes.inventory(i)?;
        i.member("maxPlanSteps")?;
        self.max_plan_steps.inventory(i)?;
        i.member("maxPlanStatements")?;
        self.max_plan_statements.inventory(i)?;
        i.member("maxPlanBytes")?;
        self.max_plan_bytes.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Schema {
    limits: Limits,
    extensions: Vec<String>,
    tables: Vec<Table>,
}
impl Decode for Schema {
    type Output = OwnedSchemaModel;
    fn finish(self) -> OwnedSchemaModel {
        OwnedSchemaModel {
            limits: self.limits.finish(),
            extensions: self.extensions.finish(),
            tables: self.tables.finish(),
        }
    }
}
impl Wire for OwnedSchemaModel {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("Schema", 3)?;
        o.serialize_field("limits", &Enc(&self.limits))?;
        o.serialize_field("extensions", &Enc(&self.extensions))?;
        o.serialize_field("tables", &Enc(&self.tables))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("limits")?;
        self.limits.inventory(i)?;
        i.member("extensions")?;
        self.extensions.inventory(i)?;
        i.member("tables")?;
        self.tables.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Table {
    entity_name: String,
    table_name: String,
    primary_key: String,
    primary_keys: Vec<String>,
    default_sort: String,
    columns: Vec<Column>,
    indexes: Vec<Index>,
    composite_unique_indexes: Vec<Vec<String>>,
    foreign_keys: Vec<ForeignKey>,
    search_indexes: Vec<Search>,
    append_only: bool,
    retention_purge: bool,
    check_constraints: Vec<Check>,
}
impl Decode for Table {
    type Output = OwnedTableModel;
    fn finish(self) -> OwnedTableModel {
        OwnedTableModel {
            entity_name: self.entity_name.finish(),
            table_name: self.table_name.finish(),
            primary_key: self.primary_key.finish(),
            primary_keys: self.primary_keys.finish(),
            default_sort: self.default_sort.finish(),
            columns: self.columns.finish(),
            indexes: self.indexes.finish(),
            composite_unique_indexes: self.composite_unique_indexes.finish(),
            foreign_keys: self.foreign_keys.finish(),
            search_indexes: self.search_indexes.finish(),
            append_only: self.append_only.finish(),
            retention_purge: self.retention_purge.finish(),
            check_constraints: self.check_constraints.finish(),
        }
    }
}
impl Wire for OwnedTableModel {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("Table", 13)?;
        o.serialize_field("entityName", &Enc(&self.entity_name))?;
        o.serialize_field("tableName", &Enc(&self.table_name))?;
        o.serialize_field("primaryKey", &Enc(&self.primary_key))?;
        o.serialize_field("primaryKeys", &Enc(&self.primary_keys))?;
        o.serialize_field("defaultSort", &Enc(&self.default_sort))?;
        o.serialize_field("columns", &Enc(&self.columns))?;
        o.serialize_field("indexes", &Enc(&self.indexes))?;
        o.serialize_field(
            "compositeUniqueIndexes",
            &Enc(&self.composite_unique_indexes),
        )?;
        o.serialize_field("foreignKeys", &Enc(&self.foreign_keys))?;
        o.serialize_field("searchIndexes", &Enc(&self.search_indexes))?;
        o.serialize_field("appendOnly", &Enc(&self.append_only))?;
        o.serialize_field("retentionPurge", &Enc(&self.retention_purge))?;
        o.serialize_field("checkConstraints", &Enc(&self.check_constraints))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("entityName")?;
        self.entity_name.inventory(i)?;
        i.member("tableName")?;
        self.table_name.inventory(i)?;
        i.member("primaryKey")?;
        self.primary_key.inventory(i)?;
        i.member("primaryKeys")?;
        self.primary_keys.inventory(i)?;
        i.member("defaultSort")?;
        self.default_sort.inventory(i)?;
        i.member("columns")?;
        self.columns.inventory(i)?;
        i.member("indexes")?;
        self.indexes.inventory(i)?;
        i.member("compositeUniqueIndexes")?;
        self.composite_unique_indexes.inventory(i)?;
        i.member("foreignKeys")?;
        self.foreign_keys.inventory(i)?;
        i.member("searchIndexes")?;
        self.search_indexes.inventory(i)?;
        i.member("appendOnly")?;
        self.append_only.inventory(i)?;
        i.member("retentionPurge")?;
        self.retention_purge.inventory(i)?;
        i.member("checkConstraints")?;
        self.check_constraints.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Column {
    name: String,
    sql_type: String,
    #[serde(deserialize_with = "required_option")]
    spatial: Option<Spatial>,
    nullable: bool,
    is_primary_key: bool,
    is_unique: bool,
    #[serde(deserialize_with = "required_option")]
    default: Option<String>,
}
impl Decode for Column {
    type Output = ColumnModel;
    fn finish(self) -> ColumnModel {
        ColumnModel {
            name: self.name.finish(),
            sql_type: self.sql_type.finish(),
            spatial: self.spatial.finish(),
            nullable: self.nullable.finish(),
            is_primary_key: self.is_primary_key.finish(),
            is_unique: self.is_unique.finish(),
            default: self.default.finish(),
        }
    }
}
impl Wire for ColumnModel {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("Column", 7)?;
        o.serialize_field("name", &Enc(&self.name))?;
        o.serialize_field("sqlType", &Enc(&self.sql_type))?;
        o.serialize_field("spatial", &Enc(&self.spatial))?;
        o.serialize_field("nullable", &Enc(&self.nullable))?;
        o.serialize_field("isPrimaryKey", &Enc(&self.is_primary_key))?;
        o.serialize_field("isUnique", &Enc(&self.is_unique))?;
        o.serialize_field("default", &Enc(&self.default))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("name")?;
        self.name.inventory(i)?;
        i.member("sqlType")?;
        self.sql_type.inventory(i)?;
        i.member("spatial")?;
        self.spatial.inventory(i)?;
        i.member("nullable")?;
        self.nullable.inventory(i)?;
        i.member("isPrimaryKey")?;
        self.is_primary_key.inventory(i)?;
        i.member("isUnique")?;
        self.is_unique.inventory(i)?;
        i.member("default")?;
        self.default.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Spatial {
    kind: SpatialKindWire,
    geometry_type: GeometryWire,
    srid: i32,
}
impl Decode for Spatial {
    type Output = SpatialColumnDef;
    fn finish(self) -> SpatialColumnDef {
        SpatialColumnDef {
            kind: self.kind.finish(),
            geometry_type: self.geometry_type.finish(),
            srid: self.srid.finish(),
        }
    }
}
impl Wire for SpatialColumnDef {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("Spatial", 3)?;
        o.serialize_field("kind", &Enc(&self.kind))?;
        o.serialize_field("geometryType", &Enc(&self.geometry_type))?;
        o.serialize_field("srid", &Enc(&self.srid))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("kind")?;
        self.kind.inventory(i)?;
        i.member("geometryType")?;
        self.geometry_type.inventory(i)?;
        i.member("srid")?;
        self.srid.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Index {
    name: String,
    columns: Vec<String>,
    column_directions: Vec<DirectionWire>,
    is_unique: bool,
    method: MethodWire,
    is_spatial: bool,
    #[serde(deserialize_with = "required_option")]
    predicate: Option<Predicate>,
}
impl Decode for Index {
    type Output = IndexModel;
    fn finish(self) -> IndexModel {
        IndexModel {
            name: self.name.finish(),
            columns: self.columns.finish(),
            column_directions: self.column_directions.finish(),
            is_unique: self.is_unique.finish(),
            method: self.method.finish(),
            is_spatial: self.is_spatial.finish(),
            predicate: self.predicate.finish(),
        }
    }
}
impl Wire for IndexModel {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("Index", 7)?;
        o.serialize_field("name", &Enc(&self.name))?;
        o.serialize_field("columns", &Enc(&self.columns))?;
        o.serialize_field("columnDirections", &Enc(&self.column_directions))?;
        o.serialize_field("isUnique", &Enc(&self.is_unique))?;
        o.serialize_field("method", &Enc(&self.method))?;
        o.serialize_field("isSpatial", &Enc(&self.is_spatial))?;
        o.serialize_field("predicate", &Enc(&self.predicate))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("name")?;
        self.name.inventory(i)?;
        i.member("columns")?;
        self.columns.inventory(i)?;
        i.member("columnDirections")?;
        self.column_directions.inventory(i)?;
        i.member("isUnique")?;
        self.is_unique.inventory(i)?;
        i.member("method")?;
        self.method.inventory(i)?;
        i.member("isSpatial")?;
        self.is_spatial.inventory(i)?;
        i.member("predicate")?;
        self.predicate.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Predicate {
    column: String,
    values: Vec<String>,
}
impl Decode for Predicate {
    type Output = super::super::super::owned_schema::IndexPredicateModel;
    fn finish(self) -> super::super::super::owned_schema::IndexPredicateModel {
        super::super::super::owned_schema::IndexPredicateModel {
            column: self.column.finish(),
            values: self.values.finish(),
        }
    }
}
impl Wire for super::super::super::owned_schema::IndexPredicateModel {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("Predicate", 2)?;
        o.serialize_field("column", &Enc(&self.column))?;
        o.serialize_field("values", &Enc(&self.values))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("column")?;
        self.column.inventory(i)?;
        i.member("values")?;
        self.values.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct ForeignKey {
    #[serde(deserialize_with = "required_option")]
    constraint_name: Option<String>,
    column_pairs: Vec<Pair>,
    target_table: String,
    is_multiple: bool,
    on_delete: DeleteWire,
}
impl Decode for ForeignKey {
    type Output = ForeignKeyModel;
    fn finish(self) -> ForeignKeyModel {
        ForeignKeyModel {
            constraint_name: self.constraint_name.finish(),
            column_pairs: self.column_pairs.finish(),
            target_table: self.target_table.finish(),
            is_multiple: self.is_multiple.finish(),
            on_delete: self.on_delete.finish(),
        }
    }
}
impl Wire for ForeignKeyModel {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("ForeignKey", 5)?;
        o.serialize_field("constraintName", &Enc(&self.constraint_name))?;
        o.serialize_field("columnPairs", &Enc(&self.column_pairs))?;
        o.serialize_field("targetTable", &Enc(&self.target_table))?;
        o.serialize_field("isMultiple", &Enc(&self.is_multiple))?;
        o.serialize_field("onDelete", &Enc(&self.on_delete))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("constraintName")?;
        self.constraint_name.inventory(i)?;
        i.member("columnPairs")?;
        self.column_pairs.inventory(i)?;
        i.member("targetTable")?;
        self.target_table.inventory(i)?;
        i.member("isMultiple")?;
        self.is_multiple.inventory(i)?;
        i.member("onDelete")?;
        self.on_delete.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Pair {
    source_column: String,
    target_column: String,
}
impl Decode for Pair {
    type Output = ForeignKeyColumnPairModel;
    fn finish(self) -> ForeignKeyColumnPairModel {
        ForeignKeyColumnPairModel {
            source_column: self.source_column.finish(),
            target_column: self.target_column.finish(),
        }
    }
}
impl Wire for ForeignKeyColumnPairModel {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("Pair", 2)?;
        o.serialize_field("sourceColumn", &Enc(&self.source_column))?;
        o.serialize_field("targetColumn", &Enc(&self.target_column))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("sourceColumn")?;
        self.source_column.inventory(i)?;
        i.member("targetColumn")?;
        self.target_column.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Check {
    name: String,
    expression: String,
}
impl Decode for Check {
    type Output = CheckConstraintModel;
    fn finish(self) -> CheckConstraintModel {
        CheckConstraintModel {
            name: self.name.finish(),
            expression: self.expression.finish(),
        }
    }
}
impl Wire for CheckConstraintModel {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("Check", 2)?;
        o.serialize_field("name", &Enc(&self.name))?;
        o.serialize_field("expression", &Enc(&self.expression))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("name")?;
        self.name.inventory(i)?;
        i.member("expression")?;
        self.expression.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Search {
    name: String,
    table_name: String,
    entity_name: String,
    primary_key: String,
    strategy: StrategyWire,
    language: String,
    tokenizer: String,
    min_token_len: usize,
    fallback_enabled: bool,
    fields: Vec<SearchField>,
    json_paths: Vec<SearchPath>,
    relations: Vec<SearchRelation>,
}
impl Decode for Search {
    type Output = SearchIndexModel;
    fn finish(self) -> SearchIndexModel {
        SearchIndexModel {
            name: self.name.finish(),
            table_name: self.table_name.finish(),
            entity_name: self.entity_name.finish(),
            primary_key: self.primary_key.finish(),
            strategy: self.strategy.finish(),
            language: self.language.finish(),
            tokenizer: self.tokenizer.finish(),
            min_token_len: self.min_token_len.finish(),
            fallback_enabled: self.fallback_enabled.finish(),
            fields: self.fields.finish(),
            json_paths: self.json_paths.finish(),
            relations: self.relations.finish(),
        }
    }
}
impl Wire for SearchIndexModel {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("Search", 12)?;
        o.serialize_field("name", &Enc(&self.name))?;
        o.serialize_field("tableName", &Enc(&self.table_name))?;
        o.serialize_field("entityName", &Enc(&self.entity_name))?;
        o.serialize_field("primaryKey", &Enc(&self.primary_key))?;
        o.serialize_field("strategy", &Enc(&self.strategy))?;
        o.serialize_field("language", &Enc(&self.language))?;
        o.serialize_field("tokenizer", &Enc(&self.tokenizer))?;
        o.serialize_field("minTokenLen", &Enc(&self.min_token_len))?;
        o.serialize_field("fallbackEnabled", &Enc(&self.fallback_enabled))?;
        o.serialize_field("fields", &Enc(&self.fields))?;
        o.serialize_field("jsonPaths", &Enc(&self.json_paths))?;
        o.serialize_field("relations", &Enc(&self.relations))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("name")?;
        self.name.inventory(i)?;
        i.member("tableName")?;
        self.table_name.inventory(i)?;
        i.member("entityName")?;
        self.entity_name.inventory(i)?;
        i.member("primaryKey")?;
        self.primary_key.inventory(i)?;
        i.member("strategy")?;
        self.strategy.inventory(i)?;
        i.member("language")?;
        self.language.inventory(i)?;
        i.member("tokenizer")?;
        self.tokenizer.inventory(i)?;
        i.member("minTokenLen")?;
        self.min_token_len.inventory(i)?;
        i.member("fallbackEnabled")?;
        self.fallback_enabled.inventory(i)?;
        i.member("fields")?;
        self.fields.inventory(i)?;
        i.member("jsonPaths")?;
        self.json_paths.inventory(i)?;
        i.member("relations")?;
        self.relations.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct SearchField {
    field_name: String,
    column_name: String,
    weight: WeightWire,
    #[serde(deserialize_with = "required_option")]
    alias: Option<String>,
    #[serde(deserialize_with = "required_option")]
    policy: Option<String>,
}
impl Decode for SearchField {
    type Output = SearchFieldModel;
    fn finish(self) -> SearchFieldModel {
        SearchFieldModel {
            field_name: self.field_name.finish(),
            column_name: self.column_name.finish(),
            weight: self.weight.finish(),
            alias: self.alias.finish(),
            policy: self.policy.finish(),
        }
    }
}
impl Wire for SearchFieldModel {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("SearchField", 5)?;
        o.serialize_field("fieldName", &Enc(&self.field_name))?;
        o.serialize_field("columnName", &Enc(&self.column_name))?;
        o.serialize_field("weight", &Enc(&self.weight))?;
        o.serialize_field("alias", &Enc(&self.alias))?;
        o.serialize_field("policy", &Enc(&self.policy))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("fieldName")?;
        self.field_name.inventory(i)?;
        i.member("columnName")?;
        self.column_name.inventory(i)?;
        i.member("weight")?;
        self.weight.inventory(i)?;
        i.member("alias")?;
        self.alias.inventory(i)?;
        i.member("policy")?;
        self.policy.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct SearchPath {
    field_name: String,
    column_name: String,
    path: String,
    weight: WeightWire,
    #[serde(deserialize_with = "required_option")]
    policy: Option<String>,
}
impl Decode for SearchPath {
    type Output = SearchJsonPathModel;
    fn finish(self) -> SearchJsonPathModel {
        SearchJsonPathModel {
            field_name: self.field_name.finish(),
            column_name: self.column_name.finish(),
            path: self.path.finish(),
            weight: self.weight.finish(),
            policy: self.policy.finish(),
        }
    }
}
impl Wire for SearchJsonPathModel {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("SearchPath", 5)?;
        o.serialize_field("fieldName", &Enc(&self.field_name))?;
        o.serialize_field("columnName", &Enc(&self.column_name))?;
        o.serialize_field("path", &Enc(&self.path))?;
        o.serialize_field("weight", &Enc(&self.weight))?;
        o.serialize_field("policy", &Enc(&self.policy))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("fieldName")?;
        self.field_name.inventory(i)?;
        i.member("columnName")?;
        self.column_name.inventory(i)?;
        i.member("path")?;
        self.path.inventory(i)?;
        i.member("weight")?;
        self.weight.inventory(i)?;
        i.member("policy")?;
        self.policy.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct SearchRelation {
    relation_field: String,
    target_type: String,
    fields: Vec<String>,
    weight: WeightWire,
    max_items: usize,
    #[serde(deserialize_with = "required_option")]
    policy: Option<String>,
}
impl Decode for SearchRelation {
    type Output = SearchRelationFieldModel;
    fn finish(self) -> SearchRelationFieldModel {
        SearchRelationFieldModel {
            relation_field: self.relation_field.finish(),
            target_type: self.target_type.finish(),
            fields: self.fields.finish(),
            weight: self.weight.finish(),
            max_items: self.max_items.finish(),
            policy: self.policy.finish(),
        }
    }
}
impl Wire for SearchRelationFieldModel {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("SearchRelation", 6)?;
        o.serialize_field("relationField", &Enc(&self.relation_field))?;
        o.serialize_field("targetType", &Enc(&self.target_type))?;
        o.serialize_field("fields", &Enc(&self.fields))?;
        o.serialize_field("weight", &Enc(&self.weight))?;
        o.serialize_field("maxItems", &Enc(&self.max_items))?;
        o.serialize_field("policy", &Enc(&self.policy))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("relationField")?;
        self.relation_field.inventory(i)?;
        i.member("targetType")?;
        self.target_type.inventory(i)?;
        i.member("fields")?;
        self.fields.inventory(i)?;
        i.member("weight")?;
        self.weight.inventory(i)?;
        i.member("maxItems")?;
        self.max_items.inventory(i)?;
        i.member("policy")?;
        self.policy.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Options {
    ignore_unmanaged_tables: bool,
}
impl Decode for Options {
    type Output = PlanOptions;
    fn finish(self) -> PlanOptions {
        PlanOptions {
            ignore_unmanaged_tables: self.ignore_unmanaged_tables.finish(),
        }
    }
}
impl Wire for PlanOptions {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("Options", 1)?;
        o.serialize_field("ignoreUnmanagedTables", &Enc(&self.ignore_unmanaged_tables))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("ignoreUnmanagedTables")?;
        self.ignore_unmanaged_tables.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Classified {
    step: Step,
    risk: RiskWire,
    reason: String,
}
impl Decode for Classified {
    type Output = OwnedPlannedMigrationStep;
    fn finish(self) -> OwnedPlannedMigrationStep {
        OwnedPlannedMigrationStep {
            step: self.step.finish(),
            risk: self.risk.finish(),
            reason: self.reason.finish(),
        }
    }
}
impl Wire for OwnedPlannedMigrationStep {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("Classified", 3)?;
        o.serialize_field("step", &Enc(&self.step))?;
        o.serialize_field("risk", &Enc(&self.risk))?;
        o.serialize_field("reason", &Enc(&self.reason))?;
        o.end()
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("step")?;
        self.step.inventory(i)?;
        i.member("risk")?;
        self.risk.inventory(i)?;
        i.member("reason")?;
        self.reason.inventory(i)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum StepObject {
    EnableExtension {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        name: String,
    },
    CreateTable {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        table: Table,
    },
    DropTable {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        table_name: String,
    },
    AddColumn {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        table_name: String,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        column: Column,
    },
    DropColumn {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        table_name: String,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        column_name: String,
    },
    AlterColumn {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        table_name: String,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        before: Column,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        after: Column,
    },
    CreateIndex {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        table_name: String,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        index: Index,
    },
    DropIndex {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        table_name: String,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        index_name: String,
    },
    CreateSearchIndex {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        table_name: String,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        index: Search,
    },
    DropSearchIndex {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        table_name: String,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        index_name: String,
    },
    AlterSearchIndex {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        table_name: String,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        before: Search,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        after: Search,
    },
    AddForeignKey {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        table_name: String,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        foreign_key: ForeignKey,
    },
    DropForeignKey {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        table_name: String,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        foreign_key: ForeignKey,
    },
    SetAppendOnly {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        table_name: String,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        enabled: bool,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        retention_purge: bool,
    },
    SetCheckConstraints {
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        table_name: String,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        before: Vec<Check>,
        #[serde(deserialize_with = "super::bounded_decode::payload")]
        after: Vec<Check>,
    },
}
impl Decode for StepObject {
    type Output = OwnedMigrationStep;
    fn finish(self) -> OwnedMigrationStep {
        match self {
            Self::EnableExtension { name } => OwnedMigrationStep::EnableExtension {
                name: name.finish(),
            },
            Self::CreateTable { table } => OwnedMigrationStep::CreateTable(table.finish()),
            Self::DropTable { table_name } => OwnedMigrationStep::DropTable {
                table_name: table_name.finish(),
            },
            Self::AddColumn { table_name, column } => OwnedMigrationStep::AddColumn {
                table_name: table_name.finish(),
                column: column.finish(),
            },
            Self::DropColumn {
                table_name,
                column_name,
            } => OwnedMigrationStep::DropColumn {
                table_name: table_name.finish(),
                column_name: column_name.finish(),
            },
            Self::AlterColumn {
                table_name,
                before,
                after,
            } => OwnedMigrationStep::AlterColumn {
                table_name: table_name.finish(),
                before: before.finish(),
                after: after.finish(),
            },
            Self::CreateIndex { table_name, index } => OwnedMigrationStep::CreateIndex {
                table_name: table_name.finish(),
                index: index.finish(),
            },
            Self::DropIndex {
                table_name,
                index_name,
            } => OwnedMigrationStep::DropIndex {
                table_name: table_name.finish(),
                index_name: index_name.finish(),
            },
            Self::CreateSearchIndex { table_name, index } => {
                OwnedMigrationStep::CreateSearchIndex {
                    table_name: table_name.finish(),
                    index: index.finish(),
                }
            }
            Self::DropSearchIndex {
                table_name,
                index_name,
            } => OwnedMigrationStep::DropSearchIndex {
                table_name: table_name.finish(),
                index_name: index_name.finish(),
            },
            Self::AlterSearchIndex {
                table_name,
                before,
                after,
            } => OwnedMigrationStep::AlterSearchIndex {
                table_name: table_name.finish(),
                before: before.finish(),
                after: after.finish(),
            },
            Self::AddForeignKey {
                table_name,
                foreign_key,
            } => OwnedMigrationStep::AddForeignKey {
                table_name: table_name.finish(),
                foreign_key: foreign_key.finish(),
            },
            Self::DropForeignKey {
                table_name,
                foreign_key,
            } => OwnedMigrationStep::DropForeignKey {
                table_name: table_name.finish(),
                foreign_key: foreign_key.finish(),
            },
            Self::SetAppendOnly {
                table_name,
                enabled,
                retention_purge,
            } => OwnedMigrationStep::SetAppendOnly {
                table_name: table_name.finish(),
                enabled: enabled.finish(),
                retention_purge: retention_purge.finish(),
            },
            Self::SetCheckConstraints {
                table_name,
                before,
                after,
            } => OwnedMigrationStep::SetCheckConstraints {
                table_name: table_name.finish(),
                before: before.finish(),
                after: after.finish(),
            },
        }
    }
}
impl Wire for OwnedMigrationStep {
    fn emit<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::EnableExtension { name } => {
                let mut o = s.serialize_struct("Step", 2)?;
                o.serialize_field("kind", "enableExtension")?;
                o.serialize_field("name", &Enc(name))?;
                o.end()
            }
            Self::CreateTable(table) => {
                let mut o = s.serialize_struct("Step", 2)?;
                o.serialize_field("kind", "createTable")?;
                o.serialize_field("table", &Enc(table))?;
                o.end()
            }
            Self::DropTable { table_name } => {
                let mut o = s.serialize_struct("Step", 2)?;
                o.serialize_field("kind", "dropTable")?;
                o.serialize_field("tableName", &Enc(table_name))?;
                o.end()
            }
            Self::AddColumn { table_name, column } => {
                let mut o = s.serialize_struct("Step", 3)?;
                o.serialize_field("kind", "addColumn")?;
                o.serialize_field("tableName", &Enc(table_name))?;
                o.serialize_field("column", &Enc(column))?;
                o.end()
            }
            Self::DropColumn {
                table_name,
                column_name,
            } => {
                let mut o = s.serialize_struct("Step", 3)?;
                o.serialize_field("kind", "dropColumn")?;
                o.serialize_field("tableName", &Enc(table_name))?;
                o.serialize_field("columnName", &Enc(column_name))?;
                o.end()
            }
            Self::AlterColumn {
                table_name,
                before,
                after,
            } => {
                let mut o = s.serialize_struct("Step", 4)?;
                o.serialize_field("kind", "alterColumn")?;
                o.serialize_field("tableName", &Enc(table_name))?;
                o.serialize_field("before", &Enc(before))?;
                o.serialize_field("after", &Enc(after))?;
                o.end()
            }
            Self::CreateIndex { table_name, index } => {
                let mut o = s.serialize_struct("Step", 3)?;
                o.serialize_field("kind", "createIndex")?;
                o.serialize_field("tableName", &Enc(table_name))?;
                o.serialize_field("index", &Enc(index))?;
                o.end()
            }
            Self::DropIndex {
                table_name,
                index_name,
            } => {
                let mut o = s.serialize_struct("Step", 3)?;
                o.serialize_field("kind", "dropIndex")?;
                o.serialize_field("tableName", &Enc(table_name))?;
                o.serialize_field("indexName", &Enc(index_name))?;
                o.end()
            }
            Self::CreateSearchIndex { table_name, index } => {
                let mut o = s.serialize_struct("Step", 3)?;
                o.serialize_field("kind", "createSearchIndex")?;
                o.serialize_field("tableName", &Enc(table_name))?;
                o.serialize_field("index", &Enc(index))?;
                o.end()
            }
            Self::DropSearchIndex {
                table_name,
                index_name,
            } => {
                let mut o = s.serialize_struct("Step", 3)?;
                o.serialize_field("kind", "dropSearchIndex")?;
                o.serialize_field("tableName", &Enc(table_name))?;
                o.serialize_field("indexName", &Enc(index_name))?;
                o.end()
            }
            Self::AlterSearchIndex {
                table_name,
                before,
                after,
            } => {
                let mut o = s.serialize_struct("Step", 4)?;
                o.serialize_field("kind", "alterSearchIndex")?;
                o.serialize_field("tableName", &Enc(table_name))?;
                o.serialize_field("before", &Enc(before))?;
                o.serialize_field("after", &Enc(after))?;
                o.end()
            }
            Self::AddForeignKey {
                table_name,
                foreign_key,
            } => {
                let mut o = s.serialize_struct("Step", 3)?;
                o.serialize_field("kind", "addForeignKey")?;
                o.serialize_field("tableName", &Enc(table_name))?;
                o.serialize_field("foreignKey", &Enc(foreign_key))?;
                o.end()
            }
            Self::DropForeignKey {
                table_name,
                foreign_key,
            } => {
                let mut o = s.serialize_struct("Step", 3)?;
                o.serialize_field("kind", "dropForeignKey")?;
                o.serialize_field("tableName", &Enc(table_name))?;
                o.serialize_field("foreignKey", &Enc(foreign_key))?;
                o.end()
            }
            Self::SetAppendOnly {
                table_name,
                enabled,
                retention_purge,
            } => {
                let mut o = s.serialize_struct("Step", 4)?;
                o.serialize_field("kind", "setAppendOnly")?;
                o.serialize_field("tableName", &Enc(table_name))?;
                o.serialize_field("enabled", &Enc(enabled))?;
                o.serialize_field("retentionPurge", &Enc(retention_purge))?;
                o.end()
            }
            Self::SetCheckConstraints {
                table_name,
                before,
                after,
            } => {
                let mut o = s.serialize_struct("Step", 4)?;
                o.serialize_field("kind", "setCheckConstraints")?;
                o.serialize_field("tableName", &Enc(table_name))?;
                o.serialize_field("before", &Enc(before))?;
                o.serialize_field("after", &Enc(after))?;
                o.end()
            }
        }
    }
    fn inventory(&self, i: &mut Inventory) -> R<()> {
        i.scalar()?;
        i.member("kind")?;
        i.scalar()?;
        match self {
            Self::EnableExtension { name } => {
                i.member("name")?;
                name.inventory(i)?;
            }
            Self::CreateTable(table) => {
                i.member("table")?;
                table.inventory(i)?;
            }
            Self::DropTable { table_name } => {
                i.member("tableName")?;
                table_name.inventory(i)?;
            }
            Self::AddColumn { table_name, column } => {
                i.member("tableName")?;
                table_name.inventory(i)?;
                i.member("column")?;
                column.inventory(i)?;
            }
            Self::DropColumn {
                table_name,
                column_name,
            } => {
                i.member("tableName")?;
                table_name.inventory(i)?;
                i.member("columnName")?;
                column_name.inventory(i)?;
            }
            Self::AlterColumn {
                table_name,
                before,
                after,
            } => {
                i.member("tableName")?;
                table_name.inventory(i)?;
                i.member("before")?;
                before.inventory(i)?;
                i.member("after")?;
                after.inventory(i)?;
            }
            Self::CreateIndex { table_name, index } => {
                i.member("tableName")?;
                table_name.inventory(i)?;
                i.member("index")?;
                index.inventory(i)?;
            }
            Self::DropIndex {
                table_name,
                index_name,
            } => {
                i.member("tableName")?;
                table_name.inventory(i)?;
                i.member("indexName")?;
                index_name.inventory(i)?;
            }
            Self::CreateSearchIndex { table_name, index } => {
                i.member("tableName")?;
                table_name.inventory(i)?;
                i.member("index")?;
                index.inventory(i)?;
            }
            Self::DropSearchIndex {
                table_name,
                index_name,
            } => {
                i.member("tableName")?;
                table_name.inventory(i)?;
                i.member("indexName")?;
                index_name.inventory(i)?;
            }
            Self::AlterSearchIndex {
                table_name,
                before,
                after,
            } => {
                i.member("tableName")?;
                table_name.inventory(i)?;
                i.member("before")?;
                before.inventory(i)?;
                i.member("after")?;
                after.inventory(i)?;
            }
            Self::AddForeignKey {
                table_name,
                foreign_key,
            } => {
                i.member("tableName")?;
                table_name.inventory(i)?;
                i.member("foreignKey")?;
                foreign_key.inventory(i)?;
            }
            Self::DropForeignKey {
                table_name,
                foreign_key,
            } => {
                i.member("tableName")?;
                table_name.inventory(i)?;
                i.member("foreignKey")?;
                foreign_key.inventory(i)?;
            }
            Self::SetAppendOnly {
                table_name,
                enabled,
                retention_purge,
            } => {
                i.member("tableName")?;
                table_name.inventory(i)?;
                i.member("enabled")?;
                enabled.inventory(i)?;
                i.member("retentionPurge")?;
                retention_purge.inventory(i)?;
            }
            Self::SetCheckConstraints {
                table_name,
                before,
                after,
            } => {
                i.member("tableName")?;
                table_name.inventory(i)?;
                i.member("before")?;
                before.inventory(i)?;
                i.member("after")?;
                after.inventory(i)?;
            }
        }
        Ok(())
    }
}

// Require an object before entering serde's internally-tagged content visitor.
pub(super) struct Step(StepObject);
impl Decode for Step {
    type Output = OwnedMigrationStep;
    fn finish(self) -> OwnedMigrationStep {
        self.0.finish()
    }
}
impl<'de> Deserialize<'de> for Step {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> serde::de::Visitor<'de> for ObjectVisitor {
            type Value = Step;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("step object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, a: A) -> Result<Step, A::Error> {
                StepObject::deserialize(serde::de::value::MapAccessDeserializer::new(a)).map(Step)
            }
        }
        d.deserialize_map(ObjectVisitor)
    }
}

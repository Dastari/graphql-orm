use super::core::{SpatialColumnDef, SpatialGeometryType};
use super::dialect::SpatialPredicate;
use crate::graphql::filters::SpatialFilter;
use geo::relate::Relate;
use std::convert::TryFrom;

fn encode_error(message: impl Into<String>) -> sqlx::Error {
    sqlx::Error::Encode(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    )))
}

fn decode_error(message: impl Into<String>) -> sqlx::Error {
    sqlx::Error::Decode(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    )))
}

fn geojson_geometry_type(value: &geojson::Geometry) -> SpatialGeometryType {
    match &value.value {
        geojson::GeometryValue::Point { .. } => SpatialGeometryType::Point,
        geojson::GeometryValue::LineString { .. } => SpatialGeometryType::LineString,
        geojson::GeometryValue::Polygon { .. } => SpatialGeometryType::Polygon,
        geojson::GeometryValue::MultiPoint { .. } => SpatialGeometryType::MultiPoint,
        geojson::GeometryValue::MultiLineString { .. } => SpatialGeometryType::MultiLineString,
        geojson::GeometryValue::MultiPolygon { .. } => SpatialGeometryType::MultiPolygon,
        geojson::GeometryValue::GeometryCollection { .. } => {
            SpatialGeometryType::GeometryCollection
        }
    }
}

fn ensure_geometry_type(
    geometry: &geojson::Geometry,
    spatial: SpatialColumnDef,
) -> Result<(), sqlx::Error> {
    let actual = geojson_geometry_type(geometry);
    if spatial.geometry_type == SpatialGeometryType::Geometry || spatial.geometry_type == actual {
        return Ok(());
    }

    Err(encode_error(format!(
        "GeoJSON geometry type {} does not match spatial field geometry_type {}",
        actual.as_sql(),
        spatial.geometry_type.as_sql()
    )))
}

fn parse_geojson_geometry(value: &serde_json::Value) -> Result<geojson::Geometry, sqlx::Error> {
    serde_json::from_value::<geojson::Geometry>(value.clone())
        .map_err(|error| decode_error(format!("invalid GeoJSON geometry: {error}")))
}

fn geometry_to_geo_types(
    geometry: geojson::Geometry,
) -> Result<geo_types::Geometry<f64>, sqlx::Error> {
    geo_types::Geometry::<f64>::try_from(geojson::GeoJson::Geometry(geometry))
        .map_err(|error| decode_error(format!("invalid GeoJSON geometry coordinates: {error}")))
}

fn value_to_geo_types(value: &serde_json::Value) -> Result<geo_types::Geometry<f64>, sqlx::Error> {
    geometry_to_geo_types(parse_geojson_geometry(value)?)
}

pub fn validate_geojson_value(
    value: &serde_json::Value,
    spatial: SpatialColumnDef,
) -> Result<(), sqlx::Error> {
    let geometry = parse_geojson_geometry(value)?;
    ensure_geometry_type(&geometry, spatial)
}

pub fn canonical_geojson_sql_value(
    value: &serde_json::Value,
    spatial: SpatialColumnDef,
) -> Result<super::core::SqlValue, sqlx::Error> {
    validate_geojson_value(value, spatial)?;
    Ok(super::core::SqlValue::Json(value.clone()))
}

pub fn spatial_predicate_matches(
    predicate: SpatialPredicate,
    left: &serde_json::Value,
    right: &serde_json::Value,
) -> Result<bool, sqlx::Error> {
    let left = value_to_geo_types(left)?;
    let right = value_to_geo_types(right)?;
    let relation = left.relate(&right);

    Ok(match predicate {
        SpatialPredicate::Equals => relation.is_equal_topo(),
        SpatialPredicate::Disjoint => relation.is_disjoint(),
        SpatialPredicate::Intersects => relation.is_intersects(),
        SpatialPredicate::Touches => relation.is_touches(),
        SpatialPredicate::Crosses => relation.is_crosses(),
        SpatialPredicate::Within => relation.is_within(),
        SpatialPredicate::Contains => relation.is_contains(),
        SpatialPredicate::Overlaps => relation.is_overlaps(),
    })
}

fn spatial_filter_predicate(
    stored: &serde_json::Value,
    input: &async_graphql::Json<serde_json::Value>,
    predicate: SpatialPredicate,
) -> Result<bool, sqlx::Error> {
    spatial_predicate_matches(predicate, stored, &input.0)
}

pub fn spatial_filter_matches_value(
    stored: Option<&serde_json::Value>,
    filter: &SpatialFilter,
    spatial: SpatialColumnDef,
) -> Result<bool, sqlx::Error> {
    if let Some(is_null) = filter.is_null {
        if stored.is_none() != is_null {
            return Ok(false);
        }
    }

    let Some(stored) = stored else {
        return Ok(filter.equals.is_none()
            && filter.disjoint.is_none()
            && filter.intersects.is_none()
            && filter.touches.is_none()
            && filter.crosses.is_none()
            && filter.within.is_none()
            && filter.contains.is_none()
            && filter.overlaps.is_none());
    };

    validate_geojson_value(stored, spatial)?;

    if let Some(input) = &filter.equals {
        validate_geojson_value(
            &input.0,
            SpatialColumnDef::geometry(SpatialGeometryType::Geometry, spatial.srid),
        )?;
        if !spatial_filter_predicate(stored, input, SpatialPredicate::Equals)? {
            return Ok(false);
        }
    }
    if let Some(input) = &filter.disjoint {
        validate_geojson_value(
            &input.0,
            SpatialColumnDef::geometry(SpatialGeometryType::Geometry, spatial.srid),
        )?;
        if !spatial_filter_predicate(stored, input, SpatialPredicate::Disjoint)? {
            return Ok(false);
        }
    }
    if let Some(input) = &filter.intersects {
        validate_geojson_value(
            &input.0,
            SpatialColumnDef::geometry(SpatialGeometryType::Geometry, spatial.srid),
        )?;
        if !spatial_filter_predicate(stored, input, SpatialPredicate::Intersects)? {
            return Ok(false);
        }
    }
    if let Some(input) = &filter.touches {
        validate_geojson_value(
            &input.0,
            SpatialColumnDef::geometry(SpatialGeometryType::Geometry, spatial.srid),
        )?;
        if !spatial_filter_predicate(stored, input, SpatialPredicate::Touches)? {
            return Ok(false);
        }
    }
    if let Some(input) = &filter.crosses {
        validate_geojson_value(
            &input.0,
            SpatialColumnDef::geometry(SpatialGeometryType::Geometry, spatial.srid),
        )?;
        if !spatial_filter_predicate(stored, input, SpatialPredicate::Crosses)? {
            return Ok(false);
        }
    }
    if let Some(input) = &filter.within {
        validate_geojson_value(
            &input.0,
            SpatialColumnDef::geometry(SpatialGeometryType::Geometry, spatial.srid),
        )?;
        if !spatial_filter_predicate(stored, input, SpatialPredicate::Within)? {
            return Ok(false);
        }
    }
    if let Some(input) = &filter.contains {
        validate_geojson_value(
            &input.0,
            SpatialColumnDef::geometry(SpatialGeometryType::Geometry, spatial.srid),
        )?;
        if !spatial_filter_predicate(stored, input, SpatialPredicate::Contains)? {
            return Ok(false);
        }
    }
    if let Some(input) = &filter.overlaps {
        validate_geojson_value(
            &input.0,
            SpatialColumnDef::geometry(SpatialGeometryType::Geometry, spatial.srid),
        )?;
        if !spatial_filter_predicate(stored, input, SpatialPredicate::Overlaps)? {
            return Ok(false);
        }
    }

    Ok(true)
}

pub fn string_filter_matches(
    value: Option<&str>,
    filter: &crate::graphql::filters::StringFilter,
) -> bool {
    if let Some(is_null) = filter.is_null {
        if value.is_none() != is_null {
            return false;
        }
    }

    let Some(value) = value else {
        return filter.eq.is_none()
            && filter.ne.is_none()
            && filter.contains.is_none()
            && filter.starts_with.is_none()
            && filter.ends_with.is_none()
            && filter.in_list.is_none()
            && filter.not_in.is_none()
            && filter.similar.is_none();
    };

    if filter
        .eq
        .as_deref()
        .is_some_and(|expected| value != expected)
    {
        return false;
    }
    if filter
        .ne
        .as_deref()
        .is_some_and(|expected| value == expected)
    {
        return false;
    }

    let lower_value = value.to_ascii_lowercase();
    if filter
        .contains
        .as_deref()
        .is_some_and(|needle| !lower_value.contains(&needle.to_ascii_lowercase()))
    {
        return false;
    }
    if filter
        .starts_with
        .as_deref()
        .is_some_and(|needle| !lower_value.starts_with(&needle.to_ascii_lowercase()))
    {
        return false;
    }
    if filter
        .ends_with
        .as_deref()
        .is_some_and(|needle| !lower_value.ends_with(&needle.to_ascii_lowercase()))
    {
        return false;
    }
    if filter
        .in_list
        .as_ref()
        .is_some_and(|list| !list.iter().any(|candidate| candidate == value))
    {
        return false;
    }
    if filter
        .not_in
        .as_ref()
        .is_some_and(|list| list.iter().any(|candidate| candidate == value))
    {
        return false;
    }
    if filter
        .similar
        .as_ref()
        .is_some_and(|similar| !lower_value.contains(&similar.value.to_ascii_lowercase()))
    {
        return false;
    }

    true
}

pub fn int_filter_matches(value: Option<i64>, filter: &crate::graphql::filters::IntFilter) -> bool {
    if let Some(is_null) = filter.is_null {
        if value.is_none() != is_null {
            return false;
        }
    }
    let Some(value) = value else {
        return filter.eq.is_none()
            && filter.ne.is_none()
            && filter.lt.is_none()
            && filter.lte.is_none()
            && filter.gt.is_none()
            && filter.gte.is_none()
            && filter.in_list.is_none()
            && filter.not_in.is_none();
    };

    if filter.eq.is_some_and(|expected| value != expected as i64) {
        return false;
    }
    if filter.ne.is_some_and(|expected| value == expected as i64) {
        return false;
    }
    if filter.lt.is_some_and(|expected| value >= expected as i64) {
        return false;
    }
    if filter.lte.is_some_and(|expected| value > expected as i64) {
        return false;
    }
    if filter.gt.is_some_and(|expected| value <= expected as i64) {
        return false;
    }
    if filter.gte.is_some_and(|expected| value < expected as i64) {
        return false;
    }
    if filter
        .in_list
        .as_ref()
        .is_some_and(|list| !list.iter().any(|candidate| value == *candidate as i64))
    {
        return false;
    }
    if filter
        .not_in
        .as_ref()
        .is_some_and(|list| list.iter().any(|candidate| value == *candidate as i64))
    {
        return false;
    }

    true
}

pub fn uuid_filter_matches(
    value: Option<uuid::Uuid>,
    filter: &crate::graphql::filters::UuidFilter,
) -> bool {
    if let Some(is_null) = filter.is_null {
        if value.is_none() != is_null {
            return false;
        }
    }
    let Some(value) = value else {
        return filter.eq.is_none()
            && filter.ne.is_none()
            && filter.in_list.is_none()
            && filter.not_in.is_none();
    };
    if filter.eq.is_some_and(|expected| value != expected) {
        return false;
    }
    if filter.ne.is_some_and(|expected| value == expected) {
        return false;
    }
    if filter
        .in_list
        .as_ref()
        .is_some_and(|list| !list.iter().any(|candidate| value == *candidate))
    {
        return false;
    }
    if filter
        .not_in
        .as_ref()
        .is_some_and(|list| list.iter().any(|candidate| value == *candidate))
    {
        return false;
    }
    true
}

pub fn bool_filter_matches(
    value: Option<bool>,
    filter: &crate::graphql::filters::BoolFilter,
) -> bool {
    if let Some(is_null) = filter.is_null {
        if value.is_none() != is_null {
            return false;
        }
    }
    let Some(value) = value else {
        return filter.eq.is_none() && filter.ne.is_none();
    };
    if filter.eq.is_some_and(|expected| value != expected) {
        return false;
    }
    if filter.ne.is_some_and(|expected| value == expected) {
        return false;
    }
    true
}

pub fn date_filter_matches(
    value: Option<&str>,
    filter: &crate::graphql::filters::DateFilter,
) -> bool {
    if let Some(is_null) = filter.is_null {
        if value.is_none() != is_null {
            return false;
        }
    }
    let Some(value) = value else {
        return filter.eq.is_none()
            && filter.ne.is_none()
            && filter.lt.is_none()
            && filter.lte.is_none()
            && filter.gt.is_none()
            && filter.gte.is_none()
            && filter.between.is_none();
    };

    if filter
        .eq
        .as_deref()
        .is_some_and(|expected| value != expected)
    {
        return false;
    }
    if filter
        .ne
        .as_deref()
        .is_some_and(|expected| value == expected)
    {
        return false;
    }
    if filter
        .lt
        .as_deref()
        .is_some_and(|expected| value >= expected)
    {
        return false;
    }
    if filter
        .lte
        .as_deref()
        .is_some_and(|expected| value > expected)
    {
        return false;
    }
    if filter
        .gt
        .as_deref()
        .is_some_and(|expected| value <= expected)
    {
        return false;
    }
    if filter
        .gte
        .as_deref()
        .is_some_and(|expected| value < expected)
    {
        return false;
    }
    if let Some(range) = &filter.between {
        if let (Some(start), Some(end)) = (&range.start, &range.end) {
            if value < start.as_str() || value > end.as_str() {
                return false;
            }
        }
    }

    true
}

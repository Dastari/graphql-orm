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
) -> crate::Result<()> {
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

fn parse_geojson_geometry(value: &serde_json::Value) -> crate::Result<geojson::Geometry> {
    serde_json::from_value::<geojson::Geometry>(value.clone())
        .map_err(|error| decode_error(format!("invalid GeoJSON geometry: {error}")))
}

fn geometry_to_geo_types(geometry: geojson::Geometry) -> crate::Result<geo_types::Geometry<f64>> {
    geo_types::Geometry::<f64>::try_from(geojson::GeoJson::Geometry(geometry))
        .map_err(|error| decode_error(format!("invalid GeoJSON geometry coordinates: {error}")))
}

fn value_to_geo_types(value: &serde_json::Value) -> crate::Result<geo_types::Geometry<f64>> {
    geometry_to_geo_types(parse_geojson_geometry(value)?)
}

/// Validate that a JSON value is a GeoJSON geometry matching the declared spatial column.
pub fn validate_geojson_value(
    value: &serde_json::Value,
    spatial: SpatialColumnDef,
) -> crate::Result<()> {
    let geometry = parse_geojson_geometry(value)?;
    ensure_geometry_type(&geometry, spatial)
}

/// Validate and convert a GeoJSON geometry into the SQL value used by SQLite fallback storage.
pub fn canonical_geojson_sql_value(
    value: &serde_json::Value,
    spatial: SpatialColumnDef,
) -> crate::Result<super::core::SqlValue> {
    validate_geojson_value(value, spatial)?;
    Ok(super::core::SqlValue::Json(value.clone()))
}

/// Evaluate one topological spatial predicate against two GeoJSON geometries.
pub fn spatial_predicate_matches(
    predicate: SpatialPredicate,
    left: &serde_json::Value,
    right: &serde_json::Value,
) -> crate::Result<bool> {
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
) -> crate::Result<bool> {
    spatial_predicate_matches(predicate, stored, &input.0)
}

/// Evaluate a generated spatial filter against an optional stored GeoJSON value.
pub fn spatial_filter_matches_value(
    stored: Option<&serde_json::Value>,
    filter: &SpatialFilter,
    spatial: SpatialColumnDef,
) -> crate::Result<bool> {
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

/// Evaluate a string filter in memory for fallback query paths.
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
            && filter.not_in.as_ref().is_none_or(Vec::is_empty)
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

/// Evaluate an integer filter in memory for fallback query paths.
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
            && filter.not_in.as_ref().is_none_or(Vec::is_empty);
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

/// Evaluate exact signed64 operands without narrowing. Empty inclusion is false;
/// empty exclusion is a no-op.
pub fn int64_filter_matches(
    value: Option<i64>,
    filter: &crate::graphql::filters::Int64Filter,
) -> Option<bool> {
    if filter.validate().is_err() {
        return Some(false);
    }
    if filter.in_list.as_ref().is_some_and(Vec::is_empty) {
        return Some(false);
    }
    if let Some(is_null) = filter.is_null {
        if value.is_none() != is_null {
            return Some(false);
        }
    }
    let Some(value) = value else {
        let has_comparison = filter.eq.is_some()
            || filter.ne.is_some()
            || filter.lt.is_some()
            || filter.lte.is_some()
            || filter.gt.is_some()
            || filter.gte.is_some()
            || filter.in_list.as_ref().is_some_and(|v| !v.is_empty())
            || filter.not_in.as_ref().is_some_and(|v| !v.is_empty());
        // isNull was evaluated first: a definite FALSE dominates UNKNOWN
        // within this atom's conjunction, exactly as SQL AND does.
        return if has_comparison { None } else { Some(true) };
    };

    if filter.eq.is_some_and(|expected| value != expected) {
        return Some(false);
    }
    if filter.ne.is_some_and(|expected| value == expected) {
        return Some(false);
    }
    if filter.lt.is_some_and(|expected| value >= expected) {
        return Some(false);
    }
    if filter.lte.is_some_and(|expected| value > expected) {
        return Some(false);
    }
    if filter.gt.is_some_and(|expected| value <= expected) {
        return Some(false);
    }
    if filter.gte.is_some_and(|expected| value < expected) {
        return Some(false);
    }
    if filter
        .in_list
        .as_ref()
        .is_some_and(|list| !list.iter().any(|candidate| value == *candidate))
    {
        return Some(false);
    }
    if filter
        .not_in
        .as_ref()
        .is_some_and(|list| list.iter().any(|candidate| value == *candidate))
    {
        return Some(false);
    }

    Some(true)
}

/// Evaluate a UUID filter in memory for fallback query paths.
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
            && filter.not_in.as_ref().is_none_or(Vec::is_empty);
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

/// Evaluate a boolean filter in memory for fallback query paths.
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

/// Evaluate a date-string filter in memory for fallback query paths.
pub fn date_filter_matches(
    value: Option<&str>,
    filter: &crate::graphql::filters::DateFilter,
) -> bool {
    date_filter_matches_at(value, filter, sqlite_calendar_today())
}

/// Evaluate a date filter at one deterministic calendar anchor.
///
/// This is public only for generated SQLite spatial-fallback code and tests.
#[doc(hidden)]
pub fn date_filter_matches_at(
    value: Option<&str>,
    filter: &crate::graphql::filters::DateFilter,
    today: chrono::NaiveDate,
) -> bool {
    date_filter_truth_at(value, filter, today) == Some(true)
}

/// Encode an opted-in SQLite timestamp as the SQLx text binder does.
#[cfg(all(feature = "sqlite", feature = "time"))]
#[doc(hidden)]
pub fn sqlite_offset_datetime_text(value: &time::OffsetDateTime) -> Option<String> {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
}

/// Evaluate a date filter with SQL's true/false/unknown semantics.
#[doc(hidden)]
pub fn date_filter_truth_at(
    value: Option<&str>,
    filter: &crate::graphql::filters::DateFilter,
    today: chrono::NaiveDate,
) -> Option<bool> {
    if filter.validate().is_err() {
        return Some(false);
    }
    if let Some(is_null) = filter.is_null {
        if value.is_none() != is_null {
            return Some(false);
        }
    }
    let Some(value) = value else {
        return if date_filter_has_value_predicate(filter) {
            None
        } else {
            Some(true)
        };
    };

    if filter
        .eq
        .as_deref()
        .is_some_and(|expected| value != expected)
    {
        return Some(false);
    }
    if filter
        .ne
        .as_deref()
        .is_some_and(|expected| value == expected)
    {
        return Some(false);
    }
    if filter
        .lt
        .as_deref()
        .is_some_and(|expected| value >= expected)
    {
        return Some(false);
    }
    if filter
        .lte
        .as_deref()
        .is_some_and(|expected| value > expected)
    {
        return Some(false);
    }
    if filter
        .gt
        .as_deref()
        .is_some_and(|expected| value <= expected)
    {
        return Some(false);
    }
    if filter
        .gte
        .as_deref()
        .is_some_and(|expected| value < expected)
    {
        return Some(false);
    }
    if let Some(range) = &filter.between {
        if value < range.start.as_str() || value > range.end.as_str() {
            return Some(false);
        }
    }

    if filter.has_calendar_predicate() {
        use crate::temporal;

        let Some(value) = temporal::parse_calendar(value) else {
            return Some(false);
        };
        let value = value.ordering_key();
        let Some(today_start) = calendar_day(today, 0) else {
            return Some(false);
        };
        let Some(tomorrow_start) = calendar_day(today, 1) else {
            return Some(false);
        };

        if filter.in_past == Some(true) && value >= today_start {
            return Some(false);
        }
        if filter.in_future == Some(true) && value < tomorrow_start {
            return Some(false);
        }
        if filter.is_today == Some(true) && !(today_start..tomorrow_start).contains(&value) {
            return Some(false);
        }
        if let Some(days) = filter.recent_days {
            let Some(lower) = calendar_day(today, 1 - days) else {
                return Some(false);
            };
            if value < lower || value >= tomorrow_start {
                return Some(false);
            }
        }
        if let Some(days) = filter.within_days {
            let Some(upper) = calendar_day(today, days) else {
                return Some(false);
            };
            if value < today_start || value >= upper {
                return Some(false);
            }
        }
        if let Some(relative) = &filter.gte_relative {
            let Some(lower) = calendar_day(today, relative.days) else {
                return Some(false);
            };
            if value < lower {
                return Some(false);
            }
        }
        if let Some(relative) = &filter.lte_relative {
            let Some(upper) = calendar_day(today, relative.days.saturating_add(1)) else {
                return Some(false);
            };
            if value >= upper {
                return Some(false);
            }
        }
    }

    Some(true)
}

fn date_filter_has_value_predicate(filter: &crate::graphql::filters::DateFilter) -> bool {
    filter.eq.is_some()
        || filter.ne.is_some()
        || filter.lt.is_some()
        || filter.lte.is_some()
        || filter.gt.is_some()
        || filter.gte.is_some()
        || filter.between.is_some()
        || filter.has_calendar_predicate()
}

fn calendar_day(date: chrono::NaiveDate, days: i32) -> Option<(i64, u32)> {
    if let Some(date) = crate::temporal::from_chrono_date(date)
        && let Some(date) = date.checked_add(time::Duration::days(i64::from(days)))
        && crate::temporal::is_default_time_year(date.year())
    {
        return Some((date.midnight().assume_utc().unix_timestamp(), 0));
    }
    let date = date.checked_add_signed(chrono::TimeDelta::days(i64::from(days)))?;
    Some((date.and_hms_opt(0, 0, 0)?.and_utc().timestamp(), 0))
}

fn sqlite_calendar_today() -> chrono::NaiveDate {
    crate::temporal::calendar_today()
}

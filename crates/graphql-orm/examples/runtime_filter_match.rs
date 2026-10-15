//! Host-neutral intent authorization before offline immutable-record matching.
//! The host must separately prove current principal authority, snapshot provenance
//! and operation/resource binding. Offline matching proves no deleted-row RLS.
use graphql_orm::graphql::orm::*;

/// Trusted current host policy; discovery or filter construction is not a grant.
pub trait FilterAuthority {
    /// Authorize every original Boolean occurrence, including implicit AND/nulls.
    fn boolean(&mut self, kind: RuntimeFilterBoolean, ignored: bool) -> bool;
    /// Authorize every field/operator occurrence, including constant-lowered ones.
    fn field(
        &mut self,
        field: &RuntimeFieldHandle,
        op: RuntimeFilterOperator,
        ignored: bool,
    ) -> bool;
}
struct Check<'a> {
    authority: &'a mut dyn FilterAuthority,
    allowed: bool,
}
impl RuntimeFilterIntentVisitor for Check<'_> {
    fn enter_boolean(&mut self, kind: RuntimeFilterBoolean, ignored: bool) {
        self.allowed &= self.authority.boolean(kind, ignored);
    }
    fn leave_boolean(&mut self, _: RuntimeFilterBoolean, _: bool) {}
    fn field(&mut self, field: &RuntimeFieldHandle, op: RuntimeFilterOperator, ignored: bool) {
        self.allowed &= self.authority.field(field, op, ignored);
    }
}
/// Match only after complete original-intent authorization by the current host.
/// The caller supplies an authoritative immutable image and an accurate storage
/// profile. A record codec cannot prove either premise.
/// # Errors
/// Returns safe checked-filter/profile/record errors; denial releases no match.
pub fn authorized_snapshot_match(
    filter: &CheckedRuntimeFilter,
    profile: RuntimeMatchProfile,
    record: &RuntimeRecord,
    authority: &mut dyn FilterAuthority,
) -> Result<bool, RuntimeFilterError> {
    let mut check = Check {
        authority,
        allowed: true,
    };
    filter.visit_intent(&mut check);
    if !check.allowed {
        return Ok(false);
    }
    filter.prepare_match(profile)?.matches(record)
}
fn main() {}

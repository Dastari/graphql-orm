#[path = "support/fixture_target.rs"]
mod fixture_target;

use std::path::Path;

#[test]
fn fixture_targets_stay_inside_the_callers_build_directory() {
    let workspace = Path::new("/workspace");
    let cwd = workspace.join("crates/graphql-orm");
    for (caller, expected) in [
        (
            Some(Path::new("/isolated/build")),
            "/isolated/build/fixtures",
        ),
        (
            Some(Path::new("relative-build")),
            "/workspace/crates/graphql-orm/relative-build/fixtures",
        ),
        (None, "/workspace/target/fixtures"),
    ] {
        assert_eq!(
            fixture_target::derive_target_dir(caller, workspace, &cwd),
            Path::new(expected),
        );
    }
}

#[test]
fn fixture_target_is_absolute() {
    assert!(fixture_target::target_dir().is_absolute());
}

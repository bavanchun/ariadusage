mod support;

use std::fs;

#[test]
fn snapshot_schema_matches_the_generated_schema() {
    let path = support::snapshot_schema_path();
    let expected = support::generated_snapshot_schema();

    if std::env::var("ARIADUSAGE_BLESS_SCHEMAS").as_deref() == Ok("1") {
        fs::create_dir_all(path.parent().expect("schema path has a parent"))
            .expect("create schema directory");
        fs::write(&path, expected).expect("write generated snapshot schema");
        return;
    }

    let actual = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "could not read {}: {error}. Run ARIADUSAGE_BLESS_SCHEMAS=1 cargo test -p ariadusage-protocol --test schema_drift",
            path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "snapshot schema drifted. Run ARIADUSAGE_BLESS_SCHEMAS=1 cargo test -p ariadusage-protocol --test schema_drift to update it."
    );
}

#[test]
fn ipc_schema_matches_the_generated_schema() {
    let path = support::ipc_schema_path();
    let expected = support::generated_ipc_schema();

    if std::env::var("ARIADUSAGE_BLESS_SCHEMAS").as_deref() == Ok("1") {
        fs::create_dir_all(path.parent().expect("schema path has a parent"))
            .expect("create schema directory");
        fs::write(&path, expected).expect("write generated IPC schema");
        return;
    }

    let actual = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "could not read {}: {error}. Run ARIADUSAGE_BLESS_SCHEMAS=1 cargo test -p ariadusage-protocol --test schema_drift",
            path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "IPC schema drifted. Run ARIADUSAGE_BLESS_SCHEMAS=1 cargo test -p ariadusage-protocol --test schema_drift to update it."
    );
}

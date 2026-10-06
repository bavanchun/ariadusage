use std::path::PathBuf;

use ariadusage_protocol::ipc::IpcMessage;
use ariadusage_protocol::snapshot::EngineSnapshot;
use schemars::generate::SchemaSettings;
use serde_json::json;

pub const SNAPSHOT_SCHEMA_ID: &str =
    "https://github.com/bavanchun/ariadusage/schemas/snapshot.v1.json";
pub const IPC_SCHEMA_ID: &str = "https://github.com/bavanchun/ariadusage/schemas/ipc.v1.json";

#[allow(dead_code)]
pub fn repository_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("crates directory")
        .parent()
        .expect("repository root")
        .to_path_buf()
}

#[allow(dead_code)]
pub fn snapshot_schema_path() -> PathBuf {
    repository_root().join("schemas/snapshot.v1.json")
}

#[allow(dead_code)]
pub fn ipc_schema_path() -> PathBuf {
    repository_root().join("schemas/ipc.v1.json")
}

#[allow(dead_code)]
pub fn examples_dir() -> PathBuf {
    repository_root().join("schemas/examples")
}

#[allow(dead_code)]
pub fn invalid_examples_dir() -> PathBuf {
    repository_root().join("schemas/examples/invalid")
}

pub fn apply_metric_invariant(schema_val: &mut serde_json::Value) {
    if let Some(defs) = schema_val.get_mut("$defs").and_then(|d| d.as_object_mut()) {
        for (_name, def) in defs.iter_mut() {
            let has_metric_props = def
                .get("properties")
                .and_then(|p| p.as_object())
                .is_some_and(|props| props.contains_key("state") && props.contains_key("value"));
            if has_metric_props {
                let Some(def_obj) = def.as_object_mut() else {
                    continue;
                };
                def_obj.insert(
                    "if".to_string(),
                    json!({
                        "properties": {
                            "state": { "enum": ["value", "stale"] }
                        },
                        "required": ["state"]
                    }),
                );
                def_obj.insert(
                    "then".to_string(),
                    json!({
                        "required": ["value"]
                    }),
                );
                def_obj.insert(
                    "else".to_string(),
                    json!({
                        "not": {
                            "required": ["value"]
                        }
                    }),
                );
            }
        }
    }
}

#[allow(dead_code)]
pub fn generated_snapshot_schema() -> String {
    let mut schema = SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<EngineSnapshot>();
    let object = schema
        .as_object_mut()
        .expect("EngineSnapshot schema is an object");
    object.insert("$id".to_owned(), json!(SNAPSHOT_SCHEMA_ID));
    let mut schema_val = serde_json::to_value(&schema).expect("schema converts to value");
    apply_metric_invariant(&mut schema_val);
    let mut contents =
        serde_json::to_string_pretty(&schema_val).expect("schema serializes to JSON");
    contents.push('\n');
    contents
}

#[allow(dead_code)]
pub fn generated_ipc_schema() -> String {
    let mut schema = SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<IpcMessage>();
    let object = schema
        .as_object_mut()
        .expect("IpcMessage schema is an object");
    object.insert("$id".to_owned(), json!(IPC_SCHEMA_ID));
    object.insert("x-status".to_owned(), json!("draft"));
    let mut schema_val = serde_json::to_value(&schema).expect("schema converts to value");
    apply_metric_invariant(&mut schema_val);
    let mut contents =
        serde_json::to_string_pretty(&schema_val).expect("schema serializes to JSON");
    contents.push('\n');
    contents
}

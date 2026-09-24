use serde_json::Value;

#[test]
fn shipped_manifest_describes_the_worker_contract() {
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["schema_version"], 2);
    assert_eq!(manifest["kind"], "bridge");
    assert_eq!(manifest["id"], "ark-markdown-bridge");
    assert_eq!(manifest["entrypoint"], "ark-markdown-bridge.exe");
    assert!(manifest["targets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|target| target["runtime"] == "worker"
            && target["os"] == serde_json::json!(["windows"])));
    let permissions = manifest["permissions"].as_array().unwrap();
    for capability in [
        "ark.read",
        "ark.write",
        "filesystem.read",
        "filesystem.write",
    ] {
        assert!(permissions
            .iter()
            .any(|permission| permission["capability"] == capability));
    }
}

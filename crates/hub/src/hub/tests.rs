use crate::task::CallerKey;

use super::*;

#[test]
fn uri_roundtrip() {
    let uri = resource_uri("shop", "cart.state");
    assert_eq!(uri, "app-mcp://shop/cart.state");
    assert_eq!(parse_resource_uri(&uri), Some(("shop", "cart.state")));
    assert_eq!(parse_resource_uri("app-mcp://shop/"), None);
    assert_eq!(parse_resource_uri("app-mcp:///x"), None);
    assert_eq!(parse_resource_uri("file:///x"), None);
}

#[test]
fn load_manifests_override_and_skip() {
    let dir =
        std::env::temp_dir().join(format!("app-mcp-hub-manifests-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a.json"),
        r#"{"manifestVersion":1,"appId":"a","name":"A1"}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("bad.json"),
        r#"{"manifestVersion":1,"appId":"apps","name":"x"}"#,
    )
    .unwrap();
    let extra = dir.join("extra.txt");
    std::fs::write(&extra, r#"{"manifestVersion":1,"appId":"a","name":"A2"}"#).unwrap();
    let ms = load_manifests(&[extra], Some(&dir), false);
    assert_eq!(ms.len(), 2);
    let shared = HubShared::new(
        HubConfig {
            manifests: ms,
            ..Default::default()
        },
        None,
    );
    assert_eq!(
        shared.registry().manifest("a").map(|m| m.meta().name.clone()),
        Some("A2".into())
    );
    // 回归（第 4f 项 d）：清单只在注册表中保存一份，不在配置中另留一份。
    assert!(shared.config.manifests.is_empty());
    assert!(load_manifests(&[], Some(&dir.join("missing")), false).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_selection_and_overview_delivery() {
    let shared = HubShared::new(HubConfig::default(), None);
    let (a, b) = (CallerKey::api(Some("a")), CallerKey::api(Some("b")));
    lock(&shared.global_selected).insert("shop".into(), "g".into());
    assert_eq!(shared.selected_for(&a, "shop").as_deref(), Some("g"));
    shared.select_for_caller(&a, "shop", "s");
    assert_eq!(shared.selected_for(&a, "shop").as_deref(), Some("s"));
    assert_eq!(shared.selected_for(&b, "shop").as_deref(), Some("g"));
    assert_eq!(shared.merged_selection(&a)["shop"], "s");
    shared.end_task(&a);
    assert_eq!(shared.selected_for(&a, "shop").as_deref(), Some("g"));
}

#[test]
fn pairing_memory() {
    let shared = HubShared::new(HubConfig::default(), None);
    assert!(!shared.is_paired("a", Some("o"), Some("t")));
    shared.remember_pairing("a", Some("o"), "t", false);
    assert!(shared.is_paired("a", None, Some("t")));
    assert!(!shared.is_paired("b", None, Some("t")));
    assert!(!shared.is_paired("a", Some("o"), None));
    shared.remember_pairing("a", Some("o"), "t2", true);
    assert!(shared.is_paired("a", Some("o"), None));
}

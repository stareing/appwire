//! v23 标准意图的机主默认表（第 16 项 N4，spec/intents.md 第 4 节）：`Hub::set_intent_defaults` / `Hub::intents`。

use std::collections::BTreeMap;
use std::ffi::c_char;

use crate::ffi_util::{AmHubStatus, FfiError, guard, req_str};
use crate::handle::AmHub;
use crate::json::to_json;
use crate::query::{hub_ref, query};

/// 替换意图默认表：`defaults_json` 为 `{"<动词>[@<主版本>]": "<工具全名>", ...}`（`{}` 清空）。
///
/// @error 不是合法 JSON 或不是字符串到字符串的对象 → `InvalidJson`（不记入 lastError）；
/// 默认表不合法 → `InvalidConfig`（之前的默认表继续生效，原因记入 `intents.lastError`）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_set_intent_defaults(hub: *mut AmHub, defaults_json: *const c_char) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let text = unsafe { req_str(defaults_json, "defaults_json") }?;
        let defaults: BTreeMap<String, String> =
            serde_json::from_str(text).map_err(|e| FfiError::json("defaults_json", e))?;
        h.hub()?
            .set_intent_defaults(defaults)
            .map_err(|e| FfiError::new(AmHubStatus::InvalidConfig, e.0.message))
    })
}

/// 生效的意图默认表与最近一次替换失败的原因（`IntentsStatus`：`{defaults, lastError?}`，同 `am_hub_status_json` 的 `intents`）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_intents_json(hub: *const AmHub, out_json: *mut *mut c_char) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe { query(hub, out_json, |_, h| to_json(&h.intents())) }
}

//! 结果缓存接入（spec/hub-api.md 3.20）：工具调用与资源读取的查询 / 存入，以及各失效点。
//!
//! 查询在策略（`hide` / `deny`）、对象锁之后、限流 / 唤醒 / 派发之前（[`crate::call`]）；时钟取 `tokio::time::Instant`。

use std::time::Duration;

use app_mcp_protocol::{CachePolicy, CacheScope, MAX_CACHE_TTL_MS, ResultStatus, ToolsInvokeResult};
use rmcp::model::ResourceContents;
use serde_json::Value;
use tokio::time::Instant;

use crate::call::ToolRun;
use crate::hub::{HubShared, lock};
use crate::mcp_convert::OutputShape;
use crate::schema::{self, SchemaCheck};
use crate::task::CallerKey;

use super::{CacheKey, CacheStatus};

/// 缓存条目的值。
#[derive(Clone, Debug)]
pub(crate) enum CachedValue {
    Tool(CachedTool),
    Resource(ResourceContents),
}

/// 缓存的工具结果：命中时据此重建与原结果相同的 MCP 结果 / `CallOutcome`。
#[derive(Clone, Debug)]
pub(crate) struct CachedTool {
    pub result: ToolsInvokeResult,
    pub output_shape: OutputShape,
    /// 产出结果的实例。
    pub instance_id: Option<String>,
    /// 距 App 产出的毫秒数（命中时填写；存入时为 0）。
    pub age_ms: u64,
}

/// 资源读取的缓存提示（MCP 出口 `resources/read` 的 `ttlMs` / `cacheScope`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ResourceCacheHint {
    /// 命中时为剩余有效期，未命中时为声明的 `ttlMs`。
    pub ttl_ms: u64,
    pub scope: CacheScope,
    /// 命中时距 App 产出的毫秒数；未命中为 `None`。
    pub age_ms: Option<u64>,
}

/// App 声明的缓存在 Hub 边界再校验一次（G-02）：`ttlMs` 不在 1..=[`MAX_CACHE_TTL_MS`] 时不缓存。
fn valid(policy: Option<CachePolicy>) -> Option<CachePolicy> {
    policy.filter(|p| p.validate().is_ok())
}

fn ttl(policy: CachePolicy) -> Duration {
    Duration::from_millis(policy.ttl_ms.min(MAX_CACHE_TTL_MS))
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// 可存入的业务状态：`done` / `noop`（`pending` / `partial` 的后续状态还会变）。
fn storable_status(status: ResultStatus) -> bool {
    matches!(status, ResultStatus::Done | ResultStatus::Noop)
}

impl HubShared {
    /// 当前的数据失效代数：调用开始时取，结束时用于判断能否存入（[`super::ResultCache::epoch`]）。
    pub(crate) fn cache_epoch(&self) -> u64 {
        lock(&self.result_cache).epoch()
    }

    pub(crate) fn cache_status(&self) -> CacheStatus {
        lock(&self.result_cache).status()
    }

    /// 查询工具结果缓存。适用：缓存开启、工具的已知定义（[`crate::registry::Registry::app_tool`]）声明了生效的 `cache`、
    /// 参数通过该定义的 inputSchema（不通过时交给原路径报 `INVALID_INPUT`）；指定实例时只命中该实例产出的结果。
    pub(crate) fn lookup_tool_cache(
        &self,
        caller: &CallerKey,
        app_id: &str,
        tool: &str,
        args: &Value,
        instance: Option<&str>,
    ) -> Option<CachedTool> {
        if !lock(&self.result_cache).enabled() {
            return None;
        }
        let def = self.registry().app_tool(app_id, tool)?;
        let policy = valid(def.effective_cache())?;
        if matches!(schema::check_json(def.input_schema_json(), args), SchemaCheck::Invalid(_)) {
            return None;
        }
        let key = CacheKey::tool(policy.scope, &caller.usage_subject(), app_id, tool, args);
        let accept = |v: &CachedValue| match v {
            CachedValue::Tool(t) => instance.is_none_or(|want| t.instance_id.as_deref() == Some(want)),
            CachedValue::Resource(_) => false,
        };
        let hit = lock(&self.result_cache).get(&key, Instant::now(), accept)?;
        let CachedValue::Tool(mut t) = hit.value else { return None };
        t.age_ms = millis(hit.age);
        tracing::info!(app_id, tool, age_ms = t.age_ms, "命中结果缓存，未转发工具调用");
        Some(t)
    }

    /// App 工具调用结束后（含失败、超时、取消）：非只读工具 → 清空该 App 的条目；结果的 `stateHints` → 清所点名资源；
    /// 之后存入可缓存的成功结果。
    ///
    /// @input invoked 实际调用的工具（改调后台替代时为替代工具）；`routed` 时不存（键属于原工具）。
    /// @input epoch 调用开始时的 [`Self::cache_epoch`]：其间发生过数据失效则不存。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn settle_tool_cache(
        &self,
        caller: &CallerKey,
        app_id: &str,
        invoked: &str,
        routed: bool,
        args: &Value,
        run: &ToolRun,
        epoch: u64,
    ) {
        let def = self.registry().app_tool(app_id, invoked);
        let read_only = def.as_ref().is_some_and(|d| d.is_read_only());
        let mut cache = lock(&self.result_cache);
        let unchanged = cache.epoch() == epoch;
        if !read_only {
            cache.clear_app(app_id);
            cache.bump_epoch();
        }
        let Ok(result) = &run.result else { return };
        if !result.state_hints.is_empty() {
            for name in &result.state_hints {
                cache.clear_resource(app_id, name);
            }
            cache.bump_epoch();
        }
        let Some(policy) = valid(def.and_then(|d| d.effective_cache())) else { return };
        if !unchanged || routed || !storable_status(result.status) {
            return;
        }
        let bytes = serde_json::to_vec(result).map_or(usize::MAX, |v| v.len());
        let key = CacheKey::tool(policy.scope, &caller.usage_subject(), app_id, invoked, args);
        let value = CachedValue::Tool(CachedTool {
            result: result.clone(),
            output_shape: run.output_shape,
            instance_id: run.instance_id.clone(),
            age_ms: 0,
        });
        cache.insert(key, value, bytes, ttl(policy), Instant::now());
    }

    /// 资源的生效缓存声明（缓存开启且已知声明给出了合法的 `cache`）。
    pub(crate) fn resource_cache_policy(&self, app_id: &str, name: &str) -> Option<CachePolicy> {
        if !lock(&self.result_cache).enabled() {
            return None;
        }
        valid(self.registry().app_resource(app_id, name)?.cache)
    }

    /// 查询资源读取缓存；命中时返回内容与提示（剩余有效期、距产出的时长）。
    pub(crate) fn lookup_resource_cache(
        &self,
        caller: &CallerKey,
        app_id: &str,
        name: &str,
        policy: CachePolicy,
    ) -> Option<(ResourceContents, ResourceCacheHint)> {
        let key = CacheKey::resource(policy.scope, &caller.usage_subject(), app_id, name);
        let hit = lock(&self.result_cache).get(&key, Instant::now(), |v| matches!(v, CachedValue::Resource(_)))?;
        let CachedValue::Resource(contents) = hit.value else { return None };
        tracing::info!(app_id, resource = name, "命中结果缓存，未转发资源读取");
        let hint = ResourceCacheHint { ttl_ms: millis(hit.remaining), scope: policy.scope, age_ms: Some(millis(hit.age)) };
        Some((contents, hint))
    }

    /// 存入资源读取结果（调用开始后发生过数据失效则不存）；返回未命中时的提示。
    pub(crate) fn store_resource_cache(
        &self,
        caller: &CallerKey,
        app_id: &str,
        name: &str,
        policy: CachePolicy,
        contents: &ResourceContents,
        epoch: u64,
    ) -> ResourceCacheHint {
        let hint = ResourceCacheHint { ttl_ms: policy.ttl_ms, scope: policy.scope, age_ms: None };
        let mut cache = lock(&self.result_cache);
        if cache.epoch() != epoch {
            return hint;
        }
        let bytes = match contents {
            ResourceContents::TextResourceContents { text, uri, .. } => text.len() + uri.len(),
            ResourceContents::BlobResourceContents { blob, uri, .. } => blob.len() + uri.len(),
            #[allow(unreachable_patterns)]
            _ => return hint,
        };
        let key = CacheKey::resource(policy.scope, &caller.usage_subject(), app_id, name);
        cache.insert(key, CachedValue::Resource(contents.clone()), bytes, ttl(policy), Instant::now());
        hint
    }

    /// 声明变化（`tools/changed` / `resources/changed`，以及声明确有变化的 `tools/sync` / `resources/sync`，见
    /// [`crate::registry::Registry::tools_declaration_differs`]）：清空该 App 的条目。
    ///
    /// @why 不加失效代数：声明变化不代表数据变化；进行中的调用按结束时的声明决定是否存入。
    pub(crate) fn invalidate_app_cache(&self, app_id: &str) {
        lock(&self.result_cache).clear_app(app_id);
    }

    /// 资源内容变化（`resources/updated`）：清该资源的条目。
    pub(crate) fn invalidate_resource_cache(&self, app_id: &str, name: &str) {
        let mut cache = lock(&self.result_cache);
        cache.clear_resource(app_id, name);
        cache.bump_epoch();
    }
}

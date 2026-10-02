// conformance_runner.cpp —— 一致性用例 runner（C++ 封装与 C ABI）。用例格式与约定见 conformance/README.md，
// 结构对照 crates/native/tests/conformance.rs：按用例 app 部分注册工具与资源，连接 fake_host（--case 模式，
// 核对全部在 fake_host 内完成），汇总各用例结论。
//
// 用法：conformance_runner <cpp|c> [fake_host 可执行文件]
//   cpp：经 app_mcp.hpp（RAII 封装；handler 以抛异常失败、Call::complete() 表示无返回值）。
//   c  ：直接调用 app_mcp.h 的 am_* 函数（am_call_fail、am_call_complete(call, NULL, …) 表示无返回值）。
//   fake_host 缺省取环境变量 APP_MCP_FAKE_HOST，再缺省 <仓库>/target/debug/examples/fake_host。
//   只跑部分用例：APP_MCP_CONFORMANCE_CASES=handshake,errors。
#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <sstream>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

#include "app_mcp.hpp"
#include "conformance_json.hpp"

#ifdef _WIN32
#define popen _popen
#define pclose _pclose
#else
#include <sys/wait.h>
#endif

#ifndef APP_MCP_REPO_ROOT
#error "需要定义 APP_MCP_REPO_ROOT（CMakeLists.txt 传入）"
#endif

namespace {

using conformance::Json;
namespace fs = std::filesystem;

/// 本 runner 支持的用例能力（`requires`），见 conformance/README.md 第 4 节。
const std::vector<std::string> kFeatures = {"toolOptions", "mutate",      "lifecycle",       "wake",       "richResult",
                                            "userAction",  "progress",    "resourceOptions", "readFailure",
                                            "surface",     "navigation",  "backgroundTool",  "backgroundNavigation",
                                            "idempotencyKey"};

// ---------------------------------------------------------------------------
// 用例字段 → SDK 枚举（协议同名字符串，spec/protocol.md 第 3 节）
// ---------------------------------------------------------------------------

AmRisk parse_risk(const Json& v) {
    static const std::map<std::string, AmRisk> table = {{"read", AM_RISK_READ},
                                                        {"write", AM_RISK_WRITE},
                                                        {"destructive", AM_RISK_DESTRUCTIVE},
                                                        {"payment", AM_RISK_PAYMENT},
                                                        {"os-sensitive", AM_RISK_OS_SENSITIVE}};
    auto it = table.find(v.str_or("write"));
    return it == table.end() ? AM_RISK_WRITE : it->second;
}

AmActivation parse_activation(const Json& v) {
    static const std::map<std::string, AmActivation> table = {{"headless", AM_ACTIVATION_HEADLESS},
                                                              {"background", AM_ACTIVATION_BACKGROUND},
                                                              {"foreground", AM_ACTIVATION_FOREGROUND}};
    auto it = table.find(v.str_or(""));
    return it == table.end() ? AM_ACTIVATION_NONE : it->second;
}

AmResultStatus parse_status(const Json& v) {
    static const std::map<std::string, AmResultStatus> table = {
        {"done", AM_RESULT_DONE}, {"pending", AM_RESULT_PENDING}, {"partial", AM_RESULT_PARTIAL}, {"noop", AM_RESULT_NOOP}};
    auto it = table.find(v.str_or("done"));
    return it == table.end() ? AM_RESULT_DONE : it->second;
}

AmLifecycleMode parse_mode(const Json& v) {
    static const std::map<std::string, AmLifecycleMode> table = {
        {"persistent", AM_LIFECYCLE_PERSISTENT}, {"idle", AM_LIFECYCLE_IDLE}, {"on-demand", AM_LIFECYCLE_ON_DEMAND}};
    auto it = table.find(v.str_or("persistent"));
    return it == table.end() ? AM_LIFECYCLE_PERSISTENT : it->second;
}

AmToolSurface parse_surface(const Json& v) { return v.str_or("app") == "view" ? AM_SURFACE_VIEW : AM_SURFACE_APP; }

/// 用例 app.visibility（visible / hidden / frozen）；未给出时 nullopt。
std::optional<AmVisibility> parse_visibility(const Json& v) {
    static const std::map<std::string, AmVisibility> table = {
        {"visible", AM_VISIBLE}, {"hidden", AM_HIDDEN}, {"frozen", AM_FROZEN}};
    auto it = table.find(v.str_or(""));
    if (it == table.end()) return std::nullopt;
    return it->second;
}

std::optional<std::string> json_text(const Json& v) {
    if (v.is_null()) return std::nullopt;
    return v.dump();
}

std::vector<std::string> strings(const Json& v) {
    std::vector<std::string> out;
    for (const auto& item : v.items()) out.push_back(item.str_or(""));
    return out;
}

uint64_t to_u64(const Json& v, uint64_t fallback) {
    auto n = v.number();
    return n ? static_cast<uint64_t>(*n) : fallback;
}

// ---------------------------------------------------------------------------
// 调用端口：handler 描述（2.1）中与 SDK 无关的部分只写一次，完成方式由各后端实现
// ---------------------------------------------------------------------------

class CallPort {
public:
    virtual ~CallPort() = default;
    virtual std::string arguments_json() const = 0;
    /// handler 上下文中的幂等键（spec/protocol.md 3.3）；没有时 nullopt。
    virtual std::optional<std::string> idempotency_key() const = 0;
    virtual bool is_cancelled() const = 0;
    virtual void progress(double progress, std::optional<double> total, const std::optional<std::string>& message) = 0;
    /// 按 handler 描述的结果部分完成调用（第一个出现的结果键，见 conformance/README.md 2.1）。
    virtual void finish(const Json& spec, uint64_t count) = 0;
};

class App;

/// 按 handler 描述执行（顺序：progress → delayMs → mutate → 结果，见 conformance/README.md 2.1）。
void run_handler(const Json& spec, uint64_t count, App& app, CallPort& call);

/// 一个用例的 App：已注册工具的声明（mutate 用）与各后端的 SDK 对象。
class App {
public:
    virtual ~App() = default;

    virtual void register_tool(const Json& decl) = 0;
    virtual void register_resource(const Json& decl) = 0;
    virtual void start() = 0;
    virtual void handle_wake(const std::string& arg) = 0;
    /// 设置导航回调（conformance/README.md 2.4）；pages 为用例的 app.navigation。
    virtual void set_navigation(const Json& pages) = 0;
    /// 后台导航（conformance/README.md 2 / 4 的 backgroundNavigation）：启动前调用。
    virtual void set_navigate_in_background(bool enabled) = 0;
    virtual void set_visibility(AmVisibility visibility) = 0;
    /// 停止客户端并等待 handler 线程结束。
    virtual void stop() = 0;

    /// handler 的 `mutate` 操作（conformance/README.md 2.3）。
    void mutate(const Json& op) {
        const std::string name = op["name"].str_or("");
        const std::string kind = op["op"].str_or("");
        if (kind == "register") {
            register_tool(op["tool"]);
        } else if (kind == "update") {
            Json decl;
            {
                std::lock_guard<std::mutex> lock(decls_mu_);
                auto it = decls_.find(name);
                if (it == decls_.end()) throw std::runtime_error("update 未知工具 " + name);
                for (const auto& [k, v] : op["set"].members()) it->second.set_or_remove(k, v);
                decl = it->second;
            }
            update_tool(name, decl);
        } else if (kind == "remove") {
            remove_tool(name);
        } else if (kind == "enable" || kind == "disable") {
            set_enabled(name, kind == "enable");
        } else {
            throw std::runtime_error("未知的 mutate 操作 " + kind);
        }
    }

    /// 工具 handler 入口（分发线程上调用）。有 delayMs 时转到工作线程（不能阻塞分发线程），其余就地执行。
    template <typename Port>
    void dispatch(const std::shared_ptr<const Json>& spec, uint64_t count, std::unique_ptr<Port> port,
                  void (*on_thread_error)(Port&, const std::exception&)) {
        if (!(*spec)["delayMs"].number()) {
            run_handler(*spec, count, *this, *port);
            return;
        }
        std::lock_guard<std::mutex> lock(workers_mu_);
        workers_.emplace_back([this, spec, count, on_thread_error, p = std::move(port)]() mutable {
            try {
                run_handler(*spec, count, *this, *p);
            } catch (const std::exception& e) {
                on_thread_error(*p, e);
            }
        });
    }

protected:
    void remember(const std::string& name, const Json& decl) {
        std::lock_guard<std::mutex> lock(decls_mu_);
        decls_[name] = decl;
    }
    void forget(const std::string& name) {
        std::lock_guard<std::mutex> lock(decls_mu_);
        decls_.erase(name);
    }
    void join_workers() {
        std::vector<std::thread> workers;
        {
            std::lock_guard<std::mutex> lock(workers_mu_);
            workers.swap(workers_);
        }
        for (auto& t : workers) t.join();
    }

    virtual void update_tool(const std::string& name, const Json& decl) = 0;
    virtual void remove_tool(const std::string& name) = 0;
    virtual void set_enabled(const std::string& name, bool enabled) = 0;

private:
    std::mutex decls_mu_;
    std::map<std::string, Json> decls_;
    std::mutex workers_mu_;
    std::vector<std::thread> workers_;
};

void run_handler(const Json& spec, uint64_t count, App& app, CallPort& call) {
    for (const auto& p : spec["progress"].items()) {
        call.progress(p["progress"].number().value_or(0.0), p["total"].number(), p["message"].str());
    }
    if (auto ms = spec["delayMs"].number()) {
        auto until = std::chrono::steady_clock::now() + std::chrono::milliseconds(static_cast<int64_t>(*ms));
        while (std::chrono::steady_clock::now() < until && !call.is_cancelled()) {
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
        }
    }
    for (const auto& op : spec["mutate"].items()) app.mutate(op);
    call.finish(spec, count);
}

/// 结果键 `return` / `echo` / `returnIdempotencyKey` / `counter` 对应的 data JSON；都没有时 nullopt（无返回值）。
std::optional<std::string> plain_data(const Json& spec, uint64_t count, const CallPort& port) {
    if (const Json* v = spec.find("return")) return v->dump();
    if (spec["echo"].boolean() == true) return port.arguments_json();
    if (spec["returnIdempotencyKey"].boolean() == true) {
        auto key = port.idempotency_key();
        return "{\"idempotencyKey\":" + (key ? Json::quote(*key) : std::string("null")) + "}";
    }
    if (spec["counter"].boolean() == true) return "{\"count\":" + std::to_string(count) + "}";
    return std::nullopt;
}

// ---------------------------------------------------------------------------
// C++ 封装后端（app_mcp.hpp）
// ---------------------------------------------------------------------------

std::optional<app_mcp::ToolAnnotations> tool_annotations(const Json& v) {
    if (!v.is_object()) return std::nullopt;
    app_mcp::ToolAnnotations a;
    a.title = v["title"].str();
    a.read_only_hint = v["readOnlyHint"].boolean();
    a.destructive_hint = v["destructiveHint"].boolean();
    a.idempotent_hint = v["idempotentHint"].boolean();
    a.open_world_hint = v["openWorldHint"].boolean();
    return a;
}

std::optional<app_mcp::ContentAnnotations> content_annotations(const Json& v) {
    if (!v.is_object()) return std::nullopt;
    app_mcp::ContentAnnotations a;
    if (v["audience"].is_array()) {
        std::vector<app_mcp::Audience> who;
        for (const auto& s : strings(v["audience"])) {
            who.push_back(s == "user" ? app_mcp::Audience::User : app_mcp::Audience::Assistant);
        }
        a.audience = who;
    }
    a.priority = v["priority"].number();
    a.last_modified = v["lastModified"].str();
    return a;
}

app_mcp::ToolOptions cpp_tool_options(const Json& decl) {
    app_mcp::ToolOptions o;
    o.input_schema_json = json_text(decl["inputSchema"]);
    o.risk = parse_risk(decl["risk"]);
    o.activation = parse_activation(decl["activation"]);
    o.title = decl["title"].str();
    o.enabled = decl["enabled"].boolean().value_or(true);
    o.annotations = tool_annotations(decl["annotations"]);
    o.output_schema_json = json_text(decl["outputSchema"]);
    o.surface = parse_surface(decl["surface"]) == AM_SURFACE_VIEW ? app_mcp::Surface::View : app_mcp::Surface::App;
    o.page = decl["page"].str();
    o.background_tool = decl["backgroundTool"].str();
    return o;
}

class CppPort final : public CallPort {
public:
    explicit CppPort(app_mcp::Call call) : call_(std::move(call)) {}

    std::string arguments_json() const override { return call_.arguments_json(); }
    std::optional<std::string> idempotency_key() const override { return call_.idempotency_key(); }
    bool is_cancelled() const override { return call_.is_cancelled(); }
    void progress(double progress, std::optional<double> total, const std::optional<std::string>& message) override {
        call_.progress(progress, total, message);
    }

    /// C++ 最自然的写法：失败抛异常（由封装的 trampoline 转为 am_call_fail*），无返回值为 Call::complete()。
    void finish(const Json& spec, uint64_t count) override {
        if (auto msg = spec["throw"].str()) throw std::runtime_error(*msg);
        if (const Json& u = spec["userAction"]; !u.is_null()) {
            throw app_mcp::UserActionRequired(u["message"].str_or(""), u["reason"].str(), u["uri"].str());
        }
        if (const Json& r = spec["result"]; r.is_object()) {
            app_mcp::CallResult result;
            result.data_json = json_text(r["data"]);
            result.state_hints = strings(r["stateHints"]);
            result.status = parse_status(r["status"]);
            result.state_resource = r["stateResource"].str();
            result.summary = r["summary"].str();
            result.annotations = content_annotations(r["annotations"]);
            call_.complete(result);
            return;
        }
        if (auto data = plain_data(spec, count, *this)) {
            call_.complete(*data);
            return;
        }
        // returnNothing（以及未声明结果）：C++ 封装的"无返回值"即 Call::complete()（缺省参数 "null"）。
        call_.complete();
    }

    /// 工作线程上抛出的异常没有 trampoline 接住：按封装的同一规则失败。
    static void on_thread_error(CppPort& port, const std::exception& e) {
        if (!port.call_.pending()) return;
        try {
            if (auto* ua = dynamic_cast<const app_mcp::UserActionRequired*>(&e)) {
                port.call_.fail_user_action(ua->what(), ua->reason(), ua->uri());
            } else if (auto* te = dynamic_cast<const app_mcp::ToolCallError*>(&e)) {
                port.call_.fail(te->kind(), te->what());
            } else {
                port.call_.fail("HANDLER_ERROR", e.what());
            }
        } catch (const app_mcp::Error&) {
            // 调用已结束（取消 / 超时 / 客户端已停止）：无需再完成。
        }
    }

private:
    app_mcp::Call call_;
};

class CppApp final : public App {
public:
    explicit CppApp(const app_mcp::ClientConfig& config) : client_(config) {}

    void register_tool(const Json& decl) override {
        auto spec = std::make_shared<const Json>(decl["handler"]);
        auto runs = std::make_shared<std::atomic<uint64_t>>(0);
        const std::string name = decl["name"].str_or("");
        app_mcp::Tool tool = client_.register_tool(
            name, decl["description"].str_or(""),
            [this, spec, runs](app_mcp::Call call) {
                uint64_t count = runs->fetch_add(1) + 1;
                dispatch(spec, count, std::make_unique<CppPort>(std::move(call)), &CppPort::on_thread_error);
            },
            cpp_tool_options(decl));
        {
            std::lock_guard<std::mutex> lock(mu_);
            tools_[name] = std::move(tool);
        }
        remember(name, decl);
    }

    void register_resource(const Json& decl) override {
        const Json spec = decl["read"];
        app_mcp::ResourceOptions options;
        options.mime_type = decl["mimeType"].str();
        options.realtime = decl["realtime"].boolean().value_or(false);
        options.annotations = content_annotations(decl["annotations"]);
        resources_.push_back(client_.register_resource(
            decl["name"].str_or(""), decl["description"].str_or(""),
            [spec](app_mcp::Read read) {
                if (const Json* v = spec.find("return")) {
                    read.complete(v->dump());
                    return;
                }
                // 失败都按 C++ 封装最自然的方式：抛出 ToolCallError / UserActionRequired / std::exception。
                if (const Json& f = spec["fail"]; !f.is_null()) {
                    throw app_mcp::ToolCallError(f["kind"].str_or("HANDLER_ERROR"), f["message"].str_or(""),
                                                 json_text(f["details"]));
                }
                if (const Json& u = spec["userAction"]; !u.is_null()) {
                    throw app_mcp::UserActionRequired(u["message"].str_or(""), u["reason"].str(), u["uri"].str());
                }
                throw std::runtime_error(spec["throw"].str_or("读取失败"));
            },
            options));
    }

    void start() override { client_.start(); }
    void handle_wake(const std::string& arg) override { client_.handle_wake(arg); }
    /// C++ 最自然的写法：出错抛异常（封装转为 NAVIGATION_FAILED），拒绝调用 Navigate::deny。
    void set_navigation(const Json& pages) override {
        client_.set_navigation_handler([this, pages](app_mcp::Navigate nav) {
            const Json* spec = pages.find(nav.page());
            if (!spec) {
                nav.fail("未知页面：" + nav.page());
                return;
            }
            if (auto msg = (*spec)["throw"].str()) throw std::runtime_error(*msg);
            for (const auto& op : (*spec)["mutate"].items()) mutate(op);
            if (auto msg = (*spec)["deny"].str()) {
                nav.deny(*msg);
            } else if (auto msg = (*spec)["fail"].str()) {
                nav.fail(*msg);
            } else if (const Json& u = (*spec)["userAction"]; !u.is_null()) {
                throw app_mcp::UserActionRequired(u["message"].str_or(""), u["reason"].str(), u["uri"].str());
            } else if ((*spec)["failParams"].boolean() == true) {
                nav.fail(nav.params_json().value_or(""));
            } else {
                nav.complete();
            }
        });
    }
    void set_navigate_in_background(bool enabled) override { client_.set_navigate_in_background(enabled); }
    void set_visibility(AmVisibility visibility) override { client_.set_visibility(visibility, false); }
    void stop() override {
        try {
            client_.stop();
        } catch (const app_mcp::Error&) {
        }
        join_workers();
    }

protected:
    void update_tool(const std::string& name, const Json& decl) override {
        std::lock_guard<std::mutex> lock(mu_);
        tools_.at(name).update(decl["description"].str_or(""), cpp_tool_options(decl));
    }
    void remove_tool(const std::string& name) override {
        {
            std::lock_guard<std::mutex> lock(mu_);
            auto it = tools_.find(name);
            if (it == tools_.end()) return;
            it->second.dispose();
            tools_.erase(it);
        }
        forget(name);
    }
    void set_enabled(const std::string& name, bool enabled) override {
        std::lock_guard<std::mutex> lock(mu_);
        tools_.at(name).set_enabled(enabled);
    }

private:
    app_mcp::Client client_;
    std::mutex mu_;
    std::map<std::string, app_mcp::Tool> tools_;
    std::vector<app_mcp::Resource> resources_;
};

app_mcp::ClientConfig cpp_config(const std::string& url, const Json& c) {
    app_mcp::ClientConfig cfg;
    cfg.app_id = "conf";
    cfg.app_name = "Conformance";
    cfg.host_url = url;
    const Json& l = c["lifecycle"];
    cfg.lifecycle.mode = parse_mode(l["mode"]);
    cfg.lifecycle.idle_timeout_ms = to_u64(l["idleTimeoutMs"], cfg.lifecycle.idle_timeout_ms);
    cfg.lifecycle.grace_ms = to_u64(l["graceMs"], cfg.lifecycle.grace_ms);
    cfg.lifecycle.merge_window_ms = to_u64(l["mergeWindowMs"], cfg.lifecycle.merge_window_ms);
    const Json& d = c["callDedup"];
    cfg.call_dedup.ttl_ms = to_u64(d["ttlMs"], cfg.call_dedup.ttl_ms);
    cfg.call_dedup.max_entries = static_cast<uint32_t>(to_u64(d["maxEntries"], cfg.call_dedup.max_entries));
    cfg.max_concurrent_calls = static_cast<uint32_t>(to_u64(c["maxConcurrentCalls"], cfg.max_concurrent_calls));
    return cfg;
}

// ---------------------------------------------------------------------------
// C ABI 后端（app_mcp.h 的 am_* 函数，不经 C++ 封装）
// ---------------------------------------------------------------------------

void check_c(AmStatus s, const char* what) {
    if (s != AM_OK) throw std::runtime_error(std::string(what) + " 失败：" + am_last_error_message());
}

/// 工具声明 → AmToolSpec + AmToolOptions；指针借用 decl 派生的字符串（由 holder 保持存活）。
struct CToolDecl {
    std::string name, description;
    std::optional<std::string> input_schema, title, annotations, output_schema, page, background_tool;
    AmToolSpec spec{};
    AmToolOptions options{};

    explicit CToolDecl(const Json& decl)
        : name(decl["name"].str_or("")),
          description(decl["description"].str_or("")),
          input_schema(json_text(decl["inputSchema"])),
          title(decl["title"].str()),
          annotations(json_text(decl["annotations"])),
          output_schema(json_text(decl["outputSchema"])),
          page(decl["page"].str()),
          background_tool(decl["backgroundTool"].str()) {
        spec.name = name.c_str();
        spec.description = description.c_str();
        spec.input_schema_json = input_schema ? input_schema->c_str() : nullptr;
        spec.risk = parse_risk(decl["risk"]);
        spec.activation = parse_activation(decl["activation"]);
        spec.title = title ? title->c_str() : nullptr;
        spec.enabled = decl["enabled"].boolean().value_or(true);
        options.struct_size = sizeof(AmToolOptions);
        options.annotations_json = annotations ? annotations->c_str() : nullptr;
        options.output_schema_json = output_schema ? output_schema->c_str() : nullptr;
        options.page = page ? page->c_str() : nullptr;
        options.surface = parse_surface(decl["surface"]);
        options.background_tool = background_tool ? background_tool->c_str() : nullptr;
    }
    CToolDecl(const CToolDecl&) = delete;
    CToolDecl& operator=(const CToolDecl&) = delete;
};

const char* c_or_null(const std::optional<std::string>& s) { return s ? s->c_str() : nullptr; }

class CPort final : public CallPort {
public:
    explicit CPort(AmCall* call) : call_(call) {}

    std::string arguments_json() const override { return am_call_arguments_json(call_); }
    std::optional<std::string> idempotency_key() const override {
        const char* key = am_call_idempotency_key(call_);
        return key ? std::optional<std::string>(key) : std::nullopt;
    }
    bool is_cancelled() const override { return am_call_is_cancelled(call_); }
    void progress(double progress, std::optional<double> total, const std::optional<std::string>& message) override {
        am_call_progress(call_, progress, total.value_or(-1.0), c_or_null(message));
    }

    /// C 最自然的写法：am_call_fail / am_call_fail_user_action / am_call_complete(_ex)；无返回值为 data_json = NULL。
    /// 已取消 / 超时时完成函数仍消费 call 并返回 AM_ERR_ALREADY_COMPLETED（忽略）。
    void finish(const Json& spec, uint64_t count) override {
        AmCall* call = call_;
        if (auto msg = spec["throw"].str()) {
            am_call_fail(call, "HANDLER_ERROR", msg->c_str());
            return;
        }
        if (const Json& u = spec["userAction"]; !u.is_null()) {
            am_call_fail_user_action(call, u["message"].str_or("").c_str(), c_or_null(u["reason"].str()),
                                     c_or_null(u["uri"].str()));
            return;
        }
        if (const Json& r = spec["result"]; r.is_object()) {
            auto data = json_text(r["data"]);
            auto hints = strings(r["stateHints"]);
            std::vector<const char*> hint_ptrs;
            for (const auto& h : hints) hint_ptrs.push_back(h.c_str());
            auto state_resource = r["stateResource"].str();
            auto summary = r["summary"].str();
            auto annotations = json_text(r["annotations"]);
            AmCallResult result{};
            result.struct_size = sizeof(AmCallResult);
            result.data_json = c_or_null(data);
            result.state_hints = hint_ptrs.empty() ? nullptr : hint_ptrs.data();
            result.state_hints_len = hint_ptrs.size();
            result.status = parse_status(r["status"]);
            result.state_resource = c_or_null(state_resource);
            result.summary = c_or_null(summary);
            result.annotations_json = c_or_null(annotations);
            am_call_complete_ex(call, &result);
            return;
        }
        if (auto data = plain_data(spec, count, *this)) {
            am_call_complete(call, data->c_str(), nullptr, 0);
            return;
        }
        // returnNothing（以及未声明结果）：C 的"无返回值"即 data_json = NULL。
        am_call_complete(call, nullptr, nullptr, 0);
    }

    static void on_thread_error(CPort& port, const std::exception& e) {
        am_call_fail(port.call_, "HANDLER_ERROR", e.what());
    }

private:
    AmCall* call_;
};

class CApp;

struct CToolContext {
    CApp* app;
    std::shared_ptr<const Json> spec;
    std::atomic<uint64_t> runs{0};
};

struct CResourceContext {
    Json spec;
};

struct CNavigationContext {
    CApp* app;
    Json pages;
};

class CApp final : public App {
public:
    CApp(const std::string& url, const Json& c) {
        AmClientConfig config{};
        config.app_id = "conf";
        config.app_name = "Conformance";
        config.host_url = url.c_str();
        config.client_kind = AM_CLIENT_NATIVE;
        config.max_concurrent_calls = static_cast<uint32_t>(to_u64(c["maxConcurrentCalls"], 0));
        AmLifecycle lc{};
        am_lifecycle_init(&lc);
        const Json& l = c["lifecycle"];
        lc.mode = parse_mode(l["mode"]);
        lc.idle_timeout_ms = to_u64(l["idleTimeoutMs"], lc.idle_timeout_ms);
        lc.grace_ms = to_u64(l["graceMs"], lc.grace_ms);
        AmClientOptions options{};
        options.struct_size = sizeof(AmClientOptions);
        options.lifecycle = &lc;
        // @compat C ABI 中 0 = 默认值、负数 = 关闭；用例中的 0 表示"不留窗口 / 关闭"。
        if (auto ms = l["mergeWindowMs"].number()) options.merge_window_ms = *ms == 0 ? -1 : static_cast<int64_t>(*ms);
        const Json& d = c["callDedup"];
        if (auto ms = d["ttlMs"].number()) options.call_dedup_ttl_ms = *ms == 0 ? -1 : static_cast<int64_t>(*ms);
        if (auto n = d["maxEntries"].number()) options.call_dedup_max_entries = *n == 0 ? -1 : static_cast<int32_t>(*n);
        check_c(am_client_new_ex(&config, nullptr, &options, &client_), "am_client_new_ex");
        check_c(am_client_root_scope(client_, &root_), "am_client_root_scope");
    }
    ~CApp() override {
        for (auto& [name, tool] : tools_) am_tool_free(tool);
        for (AmResource* r : resources_) am_resource_free(r);
        am_scope_free(root_);
        am_client_free(client_);
    }

    void register_tool(const Json& decl) override {
        CToolDecl d(decl);
        auto* ctx = new CToolContext{this, std::make_shared<const Json>(decl["handler"])};
        AmTool* tool = nullptr;
        // user_data 交给库：无论成功与否都由 free_user_data 释放。
        check_c(am_tool_register_ex(root_, &d.spec, &d.options, &CApp::on_tool, ctx,
                                    [](void* p) { delete static_cast<CToolContext*>(p); }, &tool),
                "am_tool_register_ex");
        {
            std::lock_guard<std::mutex> lock(mu_);
            tools_[d.name] = tool;
        }
        remember(d.name, decl);
    }

    void register_resource(const Json& decl) override {
        const std::string name = decl["name"].str_or("");
        const std::string description = decl["description"].str_or("");
        auto mime = decl["mimeType"].str();
        auto annotations = json_text(decl["annotations"]);
        AmResourceSpec spec{name.c_str(), description.c_str(), c_or_null(mime)};
        AmResourceOptions options{};
        options.struct_size = sizeof(AmResourceOptions);
        options.realtime = decl["realtime"].boolean().value_or(false);
        options.annotations_json = c_or_null(annotations);
        auto* ctx = new CResourceContext{decl["read"]};
        AmResource* out = nullptr;
        check_c(am_resource_register_ex(root_, &spec, &options, &CApp::on_read, ctx,
                                        [](void* p) { delete static_cast<CResourceContext*>(p); }, &out),
                "am_resource_register_ex");
        resources_.push_back(out);
    }

    void start() override { check_c(am_client_start(client_), "am_client_start"); }
    void set_navigation(const Json& pages) override {
        auto* ctx = new CNavigationContext{this, pages};
        check_c(am_client_set_navigation_handler(client_, &CApp::on_navigate, ctx,
                                                 [](void* p) { delete static_cast<CNavigationContext*>(p); }),
                "am_client_set_navigation_handler");
    }
    void set_navigate_in_background(bool enabled) override {
        check_c(am_client_set_navigate_in_background(client_, enabled), "am_client_set_navigate_in_background");
    }
    void set_visibility(AmVisibility visibility) override {
        check_c(am_client_set_visibility(client_, visibility, false), "am_client_set_visibility");
    }
    void handle_wake(const std::string& arg) override { am_client_handle_wake(client_, arg.c_str()); }
    void stop() override {
        am_client_stop(client_);
        join_workers();
    }

protected:
    void update_tool(const std::string& name, const Json& decl) override {
        CToolDecl d(decl);
        std::lock_guard<std::mutex> lock(mu_);
        check_c(am_tool_update_ex(tools_.at(name), &d.spec, &d.options), "am_tool_update_ex");
    }
    void remove_tool(const std::string& name) override {
        {
            std::lock_guard<std::mutex> lock(mu_);
            auto it = tools_.find(name);
            if (it == tools_.end()) return;
            check_c(am_tool_dispose(it->second), "am_tool_dispose");
            am_tool_free(it->second);
            tools_.erase(it);
        }
        forget(name);
    }
    void set_enabled(const std::string& name, bool enabled) override {
        std::lock_guard<std::mutex> lock(mu_);
        check_c(am_tool_set_enabled(tools_.at(name), enabled), "am_tool_set_enabled");
    }

private:
    static void on_tool(void* ud, AmCall* call) {
        auto* ctx = static_cast<CToolContext*>(ud);
        uint64_t count = ctx->runs.fetch_add(1) + 1;
        try {
            ctx->app->dispatch(ctx->spec, count, std::make_unique<CPort>(call), &CPort::on_thread_error);
        } catch (const std::exception& e) {
            // 就地执行时 runner 自身出错（如 mutate 失败）：按 handler 错误结束，便于在报告中看到原因。
            am_call_fail(call, "HANDLER_ERROR", e.what());
        }
    }

    /// C 最自然的写法：am_navigate_complete / am_navigate_fail / am_navigate_deny / am_navigate_fail_user_action；C 没有异常，`throw` 即 am_navigate_fail。
    static void on_navigate(void* ud, AmNavigate* nav) {
        auto* ctx = static_cast<CNavigationContext*>(ud);
        const std::string page = am_navigate_page(nav);
        const Json* spec = ctx->pages.find(page);
        if (!spec) {
            am_navigate_fail(nav, ("未知页面：" + page).c_str());
            return;
        }
        if (auto msg = (*spec)["throw"].str()) {
            am_navigate_fail(nav, msg->c_str());
            return;
        }
        try {
            for (const auto& op : (*spec)["mutate"].items()) ctx->app->mutate(op);
        } catch (const std::exception& e) {
            am_navigate_fail(nav, e.what());
            return;
        }
        if (auto msg = (*spec)["deny"].str()) {
            am_navigate_deny(nav, msg->c_str());
        } else if (auto msg = (*spec)["fail"].str()) {
            am_navigate_fail(nav, msg->c_str());
        } else if (const Json& u = (*spec)["userAction"]; !u.is_null()) {
            auto reason = u["reason"].str();
            auto uri = u["uri"].str();
            am_navigate_fail_user_action(nav, u["message"].str_or("").c_str(), c_or_null(reason), c_or_null(uri));
        } else if ((*spec)["failParams"].boolean() == true) {
            const char* params = am_navigate_params_json(nav);
            am_navigate_fail(nav, params ? params : "");
        } else {
            am_navigate_complete(nav);
        }
    }

    static void on_read(void* ud, AmRead* read) {
        const Json& spec = static_cast<CResourceContext*>(ud)->spec;
        if (const Json* v = spec.find("return")) {
            am_read_complete(read, v->dump().c_str());
        } else if (const Json& f = spec["fail"]; !f.is_null()) {
            auto details = json_text(f["details"]);
            am_read_fail_with_details(read, f["kind"].str_or("HANDLER_ERROR").c_str(), f["message"].str_or("").c_str(),
                                      c_or_null(details));
        } else if (const Json& u = spec["userAction"]; !u.is_null()) {
            am_read_fail_user_action(read, u["message"].str_or("").c_str(), c_or_null(u["reason"].str()),
                                     c_or_null(u["uri"].str()));
        } else {
            am_read_fail(read, "HANDLER_ERROR", spec["throw"].str_or("读取失败").c_str());
        }
    }

    AmClient* client_ = nullptr;
    AmScope* root_ = nullptr;
    std::mutex mu_;
    std::map<std::string, AmTool*> tools_;
    std::vector<AmResource*> resources_;
};

// ---------------------------------------------------------------------------
// 驱动 fake_host
// ---------------------------------------------------------------------------

std::string read_file(const fs::path& path) {
    std::ifstream in(path, std::ios::binary);
    if (!in) throw std::runtime_error("无法读取 " + path.string());
    std::ostringstream ss;
    ss << in.rdbuf();
    return ss.str();
}

std::string shell_quote(const std::string& s) { return "\"" + s + "\""; }

bool read_line(FILE* f, std::string& line) {
    line.clear();
    char buf[4096];
    while (std::fgets(buf, sizeof buf, f)) {
        line += buf;
        if (!line.empty() && line.back() == '\n') break;
    }
    if (line.empty()) return false;
    while (!line.empty() && (line.back() == '\n' || line.back() == '\r')) line.pop_back();
    return true;
}

int exit_code(int status) {
#ifdef _WIN32
    return status;
#else
    return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
#endif
}

std::unique_ptr<App> make_app(const std::string& sdk, const std::string& addr, const Json& c) {
    bool tcp = addr.find(':') != std::string::npos && addr.rfind("unix:", 0) != 0 && addr.rfind("pipe:", 0) != 0;
    std::string url = tcp ? "ws://" + addr + "/app" : addr;
    if (sdk == "c") return std::make_unique<CApp>(url, c);
    return std::make_unique<CppApp>(cpp_config(url, c));
}

struct Outcome {
    std::string status;
    std::string failures;
};

/// 跑一个用例，返回 fake_host 给出的结论。
Outcome run_case(const std::string& sdk, const std::string& fake_host, const fs::path& path, const fs::path& report_dir) {
    Json kase = Json::parse(read_file(path));
    std::vector<std::string> missing;
    for (const auto& f : strings(kase["requires"])) {
        if (std::find(kFeatures.begin(), kFeatures.end(), f) == kFeatures.end()) missing.push_back(f);
    }
    std::string cmd = shell_quote(fake_host) + " --case " + shell_quote(path.string()) + " --sdk " + sdk +
                      " --report-dir " + shell_quote(report_dir.string());
    if (!missing.empty()) {
        std::string list;
        for (const auto& m : missing) list += (list.empty() ? "" : ", ") + m;
        cmd += " --skip " + shell_quote("runner 不支持：" + list);
    }
    FILE* host = popen(cmd.c_str(), "r");
    if (!host) return {"error", "无法启动 fake_host"};

    std::unique_ptr<App> app;
    Outcome outcome{"error", "fake_host 没有输出结论"};
    std::string line;
    while (read_line(host, line)) {
        if (line.rfind("LISTENING ", 0) == 0) {
            try {
                app = make_app(sdk, line.substr(10), kase["app"]["config"]);
                for (const auto& t : kase["app"]["tools"].items()) app->register_tool(t);
                for (const auto& r : kase["app"]["resources"].items()) app->register_resource(r);
                if (kase["app"]["navigation"].is_object()) app->set_navigation(kase["app"]["navigation"]);
                if (auto b = kase["app"]["config"]["navigateInBackground"].boolean()) app->set_navigate_in_background(*b);
                if (auto v = parse_visibility(kase["app"]["visibility"])) app->set_visibility(*v);
                app->start();
            } catch (const std::exception& e) {
                std::fprintf(stderr, "[%s] 创建 / 注册失败：%s\n", sdk.c_str(), e.what());
            }
            continue;
        }
        Json v;
        try {
            v = Json::parse(line);
        } catch (const std::exception&) {
            continue;
        }
        const std::string type = v["type"].str_or("");
        if (type == "wake" && app) {
            app->handle_wake(v["arg"].str_or(""));
        } else if (type == "verdict") {
            outcome = {v["status"].str_or("error"), v["failures"].dump()};
        }
    }
    int code = exit_code(pclose(host));
    if (app) app->stop();
    if (code != 0 && outcome.status != "fail") outcome = {"error", "fake_host 退出码 " + std::to_string(code)};
    return outcome;
}

}  // namespace

int main(int argc, char** argv) {
    if (argc < 2 || (std::strcmp(argv[1], "cpp") != 0 && std::strcmp(argv[1], "c") != 0)) {
        std::fprintf(stderr, "用法：%s <cpp|c> [fake_host]\n", argv[0]);
        return 2;
    }
    const std::string sdk = argv[1];
    const fs::path root = APP_MCP_REPO_ROOT;
    std::string fake_host;
    if (argc >= 3) {
        fake_host = argv[2];
    } else if (const char* env = std::getenv("APP_MCP_FAKE_HOST"); env && *env) {
        fake_host = env;
    } else {
        fake_host = (root / "target" / "debug" / "examples" / "fake_host").string();
    }

    std::vector<std::string> only;
    if (const char* env = std::getenv("APP_MCP_CONFORMANCE_CASES"); env && *env) {
        std::stringstream ss(env);
        for (std::string id; std::getline(ss, id, ',');) only.push_back(id);
    }
    std::vector<fs::path> cases;
    for (const auto& entry : fs::directory_iterator(root / "conformance" / "cases")) {
        const fs::path& p = entry.path();
        if (p.extension() != ".json") continue;
        if (!only.empty() && std::find(only.begin(), only.end(), p.stem().string()) == only.end()) continue;
        cases.push_back(p);
    }
    std::sort(cases.begin(), cases.end());
    if (cases.empty()) {
        std::fprintf(stderr, "没有找到用例\n");
        return 1;
    }

    const fs::path report_dir = root / "target" / "conformance";
    std::vector<std::string> failed;
    for (const auto& path : cases) {
        Outcome o;
        try {
            o = run_case(sdk, fake_host, path, report_dir);
        } catch (const std::exception& e) {
            o = {"error", e.what()};
        }
        std::fprintf(stderr, "[%s] %-24s %s\n", sdk.c_str(), path.stem().string().c_str(), o.status.c_str());
        if (o.status != "pass" && o.status != "xfail" && o.status != "xpass" && o.status != "skip") {
            failed.push_back(path.string() + ": " + o.failures);
        }
    }
    if (!failed.empty()) {
        std::fprintf(stderr, "一致性用例失败：\n");
        for (const auto& f : failed) std::fprintf(stderr, "%s\n", f.c_str());
        return 1;
    }
    return 0;
}

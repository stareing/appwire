// app_mcp/errors.hpp —— 错误类型与 C 接口辅助函数（由 app_mcp.hpp 包含；直接包含 app_mcp.hpp 即可）。
#ifndef APP_MCP_ERRORS_HPP
#define APP_MCP_ERRORS_HPP

#include <atomic>
#include <charconv>
#include <cstdint>
#include <exception>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include "app_mcp.h"

#if !defined(AM_API_VERSION) || AM_API_VERSION < 3
#error "app_mcp.hpp 需要 app_mcp.h API 版本 3 或更高"
#endif

namespace app_mcp {

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

/// 库函数返回非 AM_OK 时抛出。
class Error : public std::runtime_error {
public:
    Error(AmStatus status, const std::string& message) : std::runtime_error(message), status_(status) {}
    AmStatus status() const noexcept { return status_; }

private:
    AmStatus status_;
};

/// handler 中抛出，按指定的协议错误类别（如 "USER_REJECTED"）失败。
/// details_json（可选）为结构化详情 JSON：对象的字段合并进错误的 data，其他值放在 data.details。
class ToolCallError : public std::runtime_error {
public:
    ToolCallError(std::string kind, const std::string& message,
                  std::optional<std::string> details_json = std::nullopt)
        : std::runtime_error(message), kind_(std::move(kind)), details_json_(std::move(details_json)) {}
    const std::string& kind() const noexcept { return kind_; }
    const std::optional<std::string>& details_json() const noexcept { return details_json_; }

private:
    std::string kind_;
    std::optional<std::string> details_json_;
};

/// USER_ACTION_REQUIRED 的 data.reason 建议取值（spec/protocol.md 第 4 节；也可用其他字符串）。
namespace user_action_reason {
inline constexpr const char* login = "login";            ///< 登录已过期 / 未登录
inline constexpr const char* permission = "permission";  ///< 系统权限未授予
inline constexpr const char* foreground = "foreground";  ///< 需要把 App 切到前台
inline constexpr const char* confirm = "confirm";        ///< 需要用户在 App 内确认
}  // namespace user_action_reason

/// 导航回调中抛出：拒绝本次导航（NAVIGATION_DENIED，如用户正在输入）；message 面向模型 / 用户。
class NavigationDenied : public std::runtime_error {
public:
    explicit NavigationDenied(const std::string& message) : std::runtime_error(message) {}
};

/// handler 中抛出，以 USER_ACTION_REQUIRED 失败（app_mcp.h v11；reader 中抛出同样带 reason / uri，v12；导航回调中抛出同样，v15）：
/// 需要用户本人操作后才能继续。
/// message 面向用户；reason（见 user_action_reason）与 uri（App 内入口，如深链接）可选，缺省时不出现在错误的 data 中。
class UserActionRequired : public ToolCallError {
public:
    explicit UserActionRequired(const std::string& message, std::optional<std::string> reason = std::nullopt,
                                std::optional<std::string> uri = std::nullopt)
        : ToolCallError("USER_ACTION_REQUIRED", message), reason_(std::move(reason)), uri_(std::move(uri)) {}
    const std::optional<std::string>& reason() const noexcept { return reason_; }
    const std::optional<std::string>& uri() const noexcept { return uri_; }

private:
    std::optional<std::string> reason_;
    std::optional<std::string> uri_;
};

namespace detail {

inline void check(AmStatus status) {
    if (status != AM_OK) {
        const char* msg = am_last_error_message();
        throw Error(status, msg ? msg : "");
    }
}

inline const char* c_str_or_null(const std::optional<std::string>& s) { return s ? s->c_str() : nullptr; }

/// 取走库分配的字符串并释放（即使复制时抛出异常也会释放）。
inline std::string take_string(char* s) {
    if (!s) return {};
    struct Free {
        char* p;
        ~Free() { am_string_free(p); }
    } guard{s};
    return std::string(s);
}

template <typename T>
void delete_fn(void* p) {
    delete static_cast<T*>(p);
}

/// Call / Read 的共享完成状态。原始指针只能被取走一次（原子交换），保证恰好完成一次。
template <typename Raw>
struct Pending {
    std::atomic<Raw*> raw;
    /// 分发 trampoline 正在执行时为 true：此时 handler 抛出的异常由 trampoline 负责失败。
    std::atomic<bool> dispatching{true};
    explicit Pending(Raw* r) : raw(r) {}
    Raw* take() { return raw.exchange(nullptr); }
    void put_back(Raw* r) { raw.store(r); }
};

}  // namespace detail

}  // namespace app_mcp

#endif  // APP_MCP_ERRORS_HPP

// conformance_json.hpp —— 一致性 runner 用的最小 JSON 读写（只供测试，不是 SDK 的一部分）。
//
// @why C++ SDK 本身不依赖 JSON 库（结果以 JSON 文本传给 C ABI），测试也不引入第三方库；
//      runner 只需读用例、读 fake_host 输出行、把用例中的值原样写回 JSON 文本。
// @invariant 数字保留原文（写回时与用例逐字相同）；对象保留键的顺序。
#ifndef APP_MCP_CONFORMANCE_JSON_HPP
#define APP_MCP_CONFORMANCE_JSON_HPP

#include <cctype>
#include <cstdint>
#include <cstdlib>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace conformance {

class Json {
public:
    enum class Kind { Null, Bool, Number, String, Array, Object };

    Json() = default;

    Kind kind() const noexcept { return kind_; }
    bool is_null() const noexcept { return kind_ == Kind::Null; }
    bool is_object() const noexcept { return kind_ == Kind::Object; }
    bool is_array() const noexcept { return kind_ == Kind::Array; }
    bool is_string() const noexcept { return kind_ == Kind::String; }

    /// 对象成员；不存在（或不是对象）时返回 nullptr。
    const Json* find(const std::string& key) const {
        if (kind_ != Kind::Object) return nullptr;
        for (const auto& [k, v] : members_) {
            if (k == key) return &v;
        }
        return nullptr;
    }
    /// 对象成员；不存在时返回 null 值。
    const Json& operator[](const std::string& key) const {
        const Json* v = find(key);
        return v ? *v : null_value();
    }

    std::optional<std::string> str() const {
        if (kind_ != Kind::String) return std::nullopt;
        return text_;
    }
    std::string str_or(const std::string& fallback) const { return kind_ == Kind::String ? text_ : fallback; }
    std::optional<double> number() const {
        if (kind_ != Kind::Number) return std::nullopt;
        return std::strtod(text_.c_str(), nullptr);
    }
    std::optional<bool> boolean() const {
        if (kind_ != Kind::Bool) return std::nullopt;
        return flag_;
    }
    const std::vector<Json>& items() const {
        static const std::vector<Json> empty;
        return kind_ == Kind::Array ? items_ : empty;
    }
    const std::vector<std::pair<std::string, Json>>& members() const {
        static const std::vector<std::pair<std::string, Json>> empty;
        return kind_ == Kind::Object ? members_ : empty;
    }

    /// 设置对象成员（已有则替换）；值为 null 时删除该成员。
    void set_or_remove(const std::string& key, const Json& value) {
        for (auto it = members_.begin(); it != members_.end(); ++it) {
            if (it->first == key) {
                if (value.is_null()) {
                    members_.erase(it);
                } else {
                    it->second = value;
                }
                return;
            }
        }
        if (!value.is_null()) members_.emplace_back(key, value);
    }

    std::string dump() const {
        std::string out;
        dump_to(out);
        return out;
    }

    /// @error 非法 JSON 抛出 std::runtime_error。
    static Json parse(const std::string& text) {
        Parser p{text, 0};
        Json v = p.value();
        p.skip_ws();
        if (p.pos != text.size()) throw std::runtime_error("JSON 末尾有多余内容");
        return v;
    }

    static std::string quote(const std::string& s) {
        static const char* hex = "0123456789abcdef";
        std::string out = "\"";
        for (unsigned char ch : s) {
            switch (ch) {
                case '"': out += "\\\""; break;
                case '\\': out += "\\\\"; break;
                case '\n': out += "\\n"; break;
                case '\r': out += "\\r"; break;
                case '\t': out += "\\t"; break;
                default:
                    if (ch < 0x20) {
                        out += "\\u00";
                        out.push_back(hex[ch >> 4]);
                        out.push_back(hex[ch & 0xf]);
                    } else {
                        out.push_back(static_cast<char>(ch));
                    }
            }
        }
        return out + "\"";
    }

private:
    static const Json& null_value() {
        static const Json v;
        return v;
    }

    void dump_to(std::string& out) const {
        switch (kind_) {
            case Kind::Null: out += "null"; break;
            case Kind::Bool: out += flag_ ? "true" : "false"; break;
            case Kind::Number: out += text_; break;
            case Kind::String: out += quote(text_); break;
            case Kind::Array:
                out += "[";
                for (size_t i = 0; i < items_.size(); ++i) {
                    if (i) out += ",";
                    items_[i].dump_to(out);
                }
                out += "]";
                break;
            case Kind::Object:
                out += "{";
                for (size_t i = 0; i < members_.size(); ++i) {
                    if (i) out += ",";
                    out += quote(members_[i].first);
                    out += ":";
                    members_[i].second.dump_to(out);
                }
                out += "}";
                break;
        }
    }

    struct Parser {
        const std::string& s;
        size_t pos;

        [[noreturn]] void fail(const char* what) const {
            throw std::runtime_error(std::string("JSON 解析失败：") + what + "（位置 " + std::to_string(pos) + "）");
        }
        void skip_ws() {
            while (pos < s.size() && (s[pos] == ' ' || s[pos] == '\t' || s[pos] == '\n' || s[pos] == '\r')) ++pos;
        }
        bool consume(const char* word) {
            size_t n = std::char_traits<char>::length(word);
            if (s.compare(pos, n, word) != 0) return false;
            pos += n;
            return true;
        }
        Json value() {
            skip_ws();
            if (pos >= s.size()) fail("意外的结尾");
            Json v;
            char c = s[pos];
            if (c == '{') {
                v.kind_ = Kind::Object;
                ++pos;
                skip_ws();
                if (pos < s.size() && s[pos] == '}') {
                    ++pos;
                    return v;
                }
                for (;;) {
                    skip_ws();
                    if (pos >= s.size() || s[pos] != '"') fail("期望键");
                    std::string key = string();
                    skip_ws();
                    if (pos >= s.size() || s[pos] != ':') fail("期望 ':'");
                    ++pos;
                    v.members_.emplace_back(std::move(key), value());
                    skip_ws();
                    if (pos < s.size() && s[pos] == ',') {
                        ++pos;
                        continue;
                    }
                    if (pos < s.size() && s[pos] == '}') {
                        ++pos;
                        return v;
                    }
                    fail("期望 ',' 或 '}'");
                }
            }
            if (c == '[') {
                v.kind_ = Kind::Array;
                ++pos;
                skip_ws();
                if (pos < s.size() && s[pos] == ']') {
                    ++pos;
                    return v;
                }
                for (;;) {
                    v.items_.push_back(value());
                    skip_ws();
                    if (pos < s.size() && s[pos] == ',') {
                        ++pos;
                        continue;
                    }
                    if (pos < s.size() && s[pos] == ']') {
                        ++pos;
                        return v;
                    }
                    fail("期望 ',' 或 ']'");
                }
            }
            if (c == '"') {
                v.kind_ = Kind::String;
                v.text_ = string();
                return v;
            }
            if (consume("true")) {
                v.kind_ = Kind::Bool;
                v.flag_ = true;
                return v;
            }
            if (consume("false")) {
                v.kind_ = Kind::Bool;
                return v;
            }
            if (consume("null")) return v;
            size_t start = pos;
            while (pos < s.size() && (std::isdigit(static_cast<unsigned char>(s[pos])) || s[pos] == '-' ||
                                      s[pos] == '+' || s[pos] == '.' || s[pos] == 'e' || s[pos] == 'E')) {
                ++pos;
            }
            if (pos == start) fail("非法字符");
            v.kind_ = Kind::Number;
            v.text_ = s.substr(start, pos - start);
            return v;
        }
        uint32_t hex4() {
            if (pos + 4 > s.size()) fail("\\u 不完整");
            uint32_t cp = 0;
            for (int i = 0; i < 4; ++i) {
                char h = s[pos++];
                cp <<= 4;
                if (h >= '0' && h <= '9') cp |= static_cast<uint32_t>(h - '0');
                else if (h >= 'a' && h <= 'f') cp |= static_cast<uint32_t>(h - 'a' + 10);
                else if (h >= 'A' && h <= 'F') cp |= static_cast<uint32_t>(h - 'A' + 10);
                else fail("\\u 非法");
            }
            return cp;
        }
        static void append_utf8(std::string& out, uint32_t cp) {
            if (cp < 0x80) {
                out.push_back(static_cast<char>(cp));
            } else if (cp < 0x800) {
                out.push_back(static_cast<char>(0xC0 | (cp >> 6)));
                out.push_back(static_cast<char>(0x80 | (cp & 0x3F)));
            } else if (cp < 0x10000) {
                out.push_back(static_cast<char>(0xE0 | (cp >> 12)));
                out.push_back(static_cast<char>(0x80 | ((cp >> 6) & 0x3F)));
                out.push_back(static_cast<char>(0x80 | (cp & 0x3F)));
            } else {
                out.push_back(static_cast<char>(0xF0 | (cp >> 18)));
                out.push_back(static_cast<char>(0x80 | ((cp >> 12) & 0x3F)));
                out.push_back(static_cast<char>(0x80 | ((cp >> 6) & 0x3F)));
                out.push_back(static_cast<char>(0x80 | (cp & 0x3F)));
            }
        }
        std::string string() {
            ++pos;  // 开头的引号
            std::string out;
            while (pos < s.size() && s[pos] != '"') {
                char c = s[pos++];
                if (c != '\\') {
                    out.push_back(c);
                    continue;
                }
                if (pos >= s.size()) fail("转义不完整");
                char e = s[pos++];
                switch (e) {
                    case '"': out.push_back('"'); break;
                    case '\\': out.push_back('\\'); break;
                    case '/': out.push_back('/'); break;
                    case 'b': out.push_back('\b'); break;
                    case 'f': out.push_back('\f'); break;
                    case 'n': out.push_back('\n'); break;
                    case 'r': out.push_back('\r'); break;
                    case 't': out.push_back('\t'); break;
                    case 'u': {
                        uint32_t cp = hex4();
                        if (cp >= 0xD800 && cp <= 0xDBFF && consume("\\u")) {
                            uint32_t lo = hex4();
                            cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                        }
                        append_utf8(out, cp);
                        break;
                    }
                    default: fail("未知转义");
                }
            }
            if (pos >= s.size()) fail("字符串未结束");
            ++pos;  // 结尾的引号
            return out;
        }
    };

    Kind kind_ = Kind::Null;
    bool flag_ = false;
    std::string text_;  // 字符串内容或数字原文
    std::vector<Json> items_;
    std::vector<std::pair<std::string, Json>> members_;
};

}  // namespace conformance

#endif  // APP_MCP_CONFORMANCE_JSON_HPP

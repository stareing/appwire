cmake_minimum_required(VERSION 3.16)
project(app_mcp_cpp LANGUAGES C CXX)

# Rust 构建出的 libapp_mcp（cdylib）所在目录：
#   -DAPP_MCP_LIB_DIR=... 或默认 $CARGO_TARGET_DIR/debug（再默认 <repo>/target/debug）。
get_filename_component(APP_MCP_REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)
if(NOT DEFINED APP_MCP_LIB_DIR)
  if(DEFINED ENV{CARGO_TARGET_DIR})
    set(APP_MCP_LIB_DIR "$ENV{CARGO_TARGET_DIR}/debug")
  else()
    set(APP_MCP_LIB_DIR "${APP_MCP_REPO_ROOT}/target/debug")
  endif()
endif()
set(APP_MCP_LIB_DIR "${APP_MCP_LIB_DIR}" CACHE PATH "libapp_mcp 所在目录")
set(APP_MCP_C_INCLUDE_DIR "${APP_MCP_REPO_ROOT}/bindings/c/include")

if(WIN32)
  set(_app_mcp_imported "${APP_MCP_LIB_DIR}/app_mcp.dll")
  set(_app_mcp_implib "${APP_MCP_LIB_DIR}/app_mcp.dll.lib")
elseif(APPLE)
  set(_app_mcp_imported "${APP_MCP_LIB_DIR}/libapp_mcp.dylib")
else()
  set(_app_mcp_imported "${APP_MCP_LIB_DIR}/libapp_mcp.so")
endif()

if(NOT EXISTS "${_app_mcp_imported}")
  message(WARNING "未找到 ${_app_mcp_imported}，请先运行 cargo build -p app-mcp-c")
endif()

# C 接口（导入的共享库）
add_library(app_mcp SHARED IMPORTED GLOBAL)
set_target_properties(app_mcp PROPERTIES
  IMPORTED_LOCATION "${_app_mcp_imported}"
  INTERFACE_INCLUDE_DIRECTORIES "${APP_MCP_C_INCLUDE_DIR}")
if(WIN32)
  set_target_properties(app_mcp PROPERTIES IMPORTED_IMPLIB "${_app_mcp_implib}")
endif()
# @why 头文件与示例含中文（UTF-8 无 BOM）；MSVC 默认按系统代码页（如 936）解码，注释吞掉换行、字面量编码错乱，
#      因此源字符集与执行字符集都要指定 UTF-8，并随目标传给使用方。
set_property(TARGET app_mcp APPEND PROPERTY INTERFACE_COMPILE_OPTIONS
  "$<$<COMPILE_LANG_AND_ID:C,MSVC>:/utf-8>" "$<$<COMPILE_LANG_AND_ID:CXX,MSVC>:/utf-8>")

# C++ header-only 封装
add_library(app_mcp_cpp INTERFACE)
target_include_directories(app_mcp_cpp INTERFACE "${CMAKE_CURRENT_SOURCE_DIR}/include")
target_link_libraries(app_mcp_cpp INTERFACE app_mcp)
target_compile_features(app_mcp_cpp INTERFACE cxx_std_17)

set(CMAKE_BUILD_RPATH "${APP_MCP_LIB_DIR}")

if(MSVC)
  set(_warn /W4)
else()
  set(_warn -Wall -Wextra -Wpedantic)
endif()

add_executable(hello_c examples/hello.c)
set_target_properties(hello_c PROPERTIES C_STANDARD 11)
target_link_libraries(hello_c PRIVATE app_mcp)
target_compile_options(hello_c PRIVATE ${_warn})
if(NOT WIN32)
  target_compile_definitions(hello_c PRIVATE _POSIX_C_SOURCE=200809L)
endif()

add_executable(hello_cpp examples/hello.cpp)
target_link_libraries(hello_cpp PRIVATE app_mcp_cpp)
target_compile_options(hello_cpp PRIVATE ${_warn})

add_executable(lifecycle_cpp examples/lifecycle.cpp)
target_link_libraries(lifecycle_cpp PRIVATE app_mcp_cpp)
target_compile_options(lifecycle_cpp PRIVATE ${_warn})

# 按名寻址示例（app_mcp.h v17，spec/naming.md）
add_executable(named_cpp examples/named.cpp)
target_link_libraries(named_cpp PRIVATE app_mcp_cpp)
target_compile_options(named_cpp PRIVATE ${_warn})

add_executable(test_app_mcp tests/test_app_mcp.cpp)
target_link_libraries(test_app_mcp PRIVATE app_mcp_cpp)
target_compile_options(test_app_mcp PRIVATE ${_warn})

enable_testing()
add_test(NAME unit COMMAND test_app_mcp)

# 集成测试：需要 fake_host（cargo run -p app-mcp-native --example fake_host）。
find_program(BASH_PROGRAM bash)
if(BASH_PROGRAM)
  add_test(NAME integration_c
    COMMAND ${BASH_PROGRAM} "${CMAKE_CURRENT_SOURCE_DIR}/tests/integration.sh" "$<TARGET_FILE:hello_c>")
  add_test(NAME integration_cpp
    COMMAND ${BASH_PROGRAM} "${CMAKE_CURRENT_SOURCE_DIR}/tests/integration.sh" "$<TARGET_FILE:hello_cpp>")
  set_tests_properties(integration_c integration_cpp PROPERTIES LABELS integration)

  # 生命周期往返：idle 休眠 → handle_wake 回连（跳过 sync）→ 调用 → 再休眠。
  add_executable(lifecycle_c tests/lifecycle_c.c)
  set_target_properties(lifecycle_c PROPERTIES C_STANDARD 11)
  target_link_libraries(lifecycle_c PRIVATE app_mcp)
  target_compile_options(lifecycle_c PRIVATE ${_warn})
  if(MSVC)
    # @why MSVC 的 <stdatomic.h> 需显式开启 C11 原子支持（否则 #error "C atomic support is not enabled"）。
    target_compile_options(lifecycle_c PRIVATE /experimental:c11atomics)
  endif()
  if(NOT WIN32)
    target_compile_definitions(lifecycle_c PRIVATE _POSIX_C_SOURCE=200809L)
  endif()
  add_executable(lifecycle_cpp_test tests/lifecycle_cpp.cpp)
  target_link_libraries(lifecycle_cpp_test PRIVATE app_mcp_cpp)
  target_compile_options(lifecycle_cpp_test PRIVATE ${_warn})
  if(NOT WIN32)
    target_compile_definitions(lifecycle_cpp_test PRIVATE _POSIX_C_SOURCE=200809L)
  endif()
  add_test(NAME lifecycle_c
    COMMAND ${BASH_PROGRAM} "${CMAKE_CURRENT_SOURCE_DIR}/tests/with_fake_host.sh" "$<TARGET_FILE:lifecycle_c>")
  add_test(NAME lifecycle_cpp
    COMMAND ${BASH_PROGRAM} "${CMAKE_CURRENT_SOURCE_DIR}/tests/with_fake_host.sh" "$<TARGET_FILE:lifecycle_cpp_test>")
  set_tests_properties(lifecycle_c lifecycle_cpp PROPERTIES LABELS "integration;lifecycle" TIMEOUT 120)

  # 经本地 IPC 正向连接：Windows 每进程独立命名管道、其他平台临时目录下的 Unix 套接字（不用默认端点）。
  add_executable(ipc_cpp_test tests/ipc_cpp.cpp)
  target_link_libraries(ipc_cpp_test PRIVATE app_mcp_cpp)
  target_compile_options(ipc_cpp_test PRIVATE ${_warn})
  if(NOT WIN32)
    target_compile_definitions(ipc_cpp_test PRIVATE _POSIX_C_SOURCE=200809L)
  endif()
  add_test(NAME ipc_cpp
    COMMAND ${BASH_PROGRAM} "${CMAKE_CURRENT_SOURCE_DIR}/tests/with_fake_host.sh" "$<TARGET_FILE:ipc_cpp_test>")
  set_tests_properties(ipc_cpp PROPERTIES LABELS "integration;ipc" TIMEOUT 120)

  # 工具声明（注解、outputSchema）与结构化调用结果（app_mcp.h v9）对接 fake_host --tool-info。
  add_executable(result_cpp_test tests/result_cpp.cpp)
  target_link_libraries(result_cpp_test PRIVATE app_mcp_cpp)
  target_compile_options(result_cpp_test PRIVATE ${_warn})
  if(NOT WIN32)
    target_compile_definitions(result_cpp_test PRIVATE _POSIX_C_SOURCE=200809L)
  endif()
  add_test(NAME result_cpp
    COMMAND ${BASH_PROGRAM} "${CMAKE_CURRENT_SOURCE_DIR}/tests/with_fake_host.sh" "$<TARGET_FILE:result_cpp_test>")
  set_tests_properties(result_cpp PROPERTIES LABELS "integration;result" TIMEOUT 120)

  # 按名寻址 Linux 全链路：私有 D-Bus 会话总线 + app-mcp-host（app install、stdio --name-service）+ named_cpp 冷激活。
  # 非 Linux 或没有 dbus-daemon 时跳过（退出码 77）。
  add_test(NAME naming_e2e
    COMMAND ${BASH_PROGRAM} "${CMAKE_CURRENT_SOURCE_DIR}/tests/naming_e2e.sh" "$<TARGET_FILE:named_cpp>")
  set_tests_properties(naming_e2e PROPERTIES LABELS "integration;naming" TIMEOUT 180 SKIP_RETURN_CODE 77)
endif()

# 一致性用例 runner（conformance/README.md）：同一程序以 cpp（C++ 封装）与 c（直接调 C ABI）两种方式跑全部用例，
# 报告写到 <repo>/target/conformance/<cpp|c>/。
add_executable(conformance_runner tests/conformance_runner.cpp)
target_link_libraries(conformance_runner PRIVATE app_mcp_cpp)
target_compile_options(conformance_runner PRIVATE ${_warn})
target_compile_definitions(conformance_runner PRIVATE "APP_MCP_REPO_ROOT=\"${APP_MCP_REPO_ROOT}\"")
if(NOT WIN32)
  target_compile_definitions(conformance_runner PRIVATE _POSIX_C_SOURCE=200809L)
  find_package(Threads REQUIRED)
  target_link_libraries(conformance_runner PRIVATE Threads::Threads)
endif()
if(BASH_PROGRAM)
  add_test(NAME conformance_cpp
    COMMAND ${BASH_PROGRAM} "${CMAKE_CURRENT_SOURCE_DIR}/tests/with_fake_host.sh" "$<TARGET_FILE:conformance_runner>" cpp)
  add_test(NAME conformance_c
    COMMAND ${BASH_PROGRAM} "${CMAKE_CURRENT_SOURCE_DIR}/tests/with_fake_host.sh" "$<TARGET_FILE:conformance_runner>" c)
  set_tests_properties(conformance_cpp conformance_c PROPERTIES LABELS "integration;conformance" TIMEOUT 300)
endif()

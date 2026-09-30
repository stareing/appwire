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
endif()

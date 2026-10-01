# 示例 App 自身不需要额外规则：app-mcp / app-mcp-hub / app-mcp-android 的规则随依赖的 jar / AAR 自动生效。
# 保留行号便于排查混淆后的崩溃栈（mapping 在 build/outputs/mapping/release/）。
-keepattributes SourceFile,LineNumberTable
-renamesourcefileattribute SourceFile

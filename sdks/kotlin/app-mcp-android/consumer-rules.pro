# app-mcp-android 的 consumer 规则（随 AAR 发布，依赖方的 R8 自动应用）。
#
# JNA 与 uniffi 绑定（dev.appmcp.ffi）的规则随 :app-mcp 的 jar 发布（META-INF/proguard/app-mcp.pro）。
# WakeReceiver 在本库 manifest 中声明，AAPT 生成的规则会保留它。

# WakeWorker：WorkManager 把 Worker 类名持久化到自己的数据库，再按名字反射创建。
# 类名被混淆后 App 升级时旧名字找不到，已入队的唤醒任务会失败，所以名字必须稳定。
# work-runtime 自带的规则目前也会 keepnames 所有 Worker；这里显式声明，不依赖其实现细节。
-keep class dev.appmcp.android.WakeWorker { <init>(android.content.Context, androidx.work.WorkerParameters); }

# app-mcp（App 端 SDK）的 R8 / ProGuard 规则。放在 jar 的 META-INF/proguard/ 下，
# Android 构建（R8）与 ProGuard 会自动应用，依赖方无需手动添加。
#
# uniffi 生成的绑定（dev.appmcp.ffi）经 JNA 调用 libapp_mcp_uniffi.so，JNA 大量依赖反射与 JNI 名字：

# JNA 本身：libjnidispatch 以 JNI 名字访问 com.sun.jna 的类、字段与方法。
-keep class com.sun.jna.** { *; }
-dontwarn java.awt.**

# Structure：按 @Structure.FieldOrder（运行时注解）列出的名字反射取字段，ByValue / ByReference 子类经无参构造创建。
-keepattributes RuntimeVisibleAnnotations
-keep class dev.appmcp.ffi.** extends com.sun.jna.Structure { *; }

# 回调：JNA 反射查找 Callback 子接口的唯一方法生成本地桩，再在实现类上按同名同签名调用；
# 接口不能被合并或改名。
-keep class dev.appmcp.ffi.** implements com.sun.jna.Callback { *; }

# 直接映射：Native.register(UniffiLib::class.java, ...) 按 native 方法名绑定 .so 中的同名符号，
# 参数 / 返回类型（Structure.ByValue、Callback、Pointer）也须原样保留。
-keepclasseswithmembers,includedescriptorclasses class dev.appmcp.ffi.** { native <methods>; }

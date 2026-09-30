# JNA 通过反射访问这些类；uniffi 生成代码通过 JNA direct mapping 注册 native 方法。
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }
-keep class dev.appmcp.ffi.** { *; }
-dontwarn java.awt.**

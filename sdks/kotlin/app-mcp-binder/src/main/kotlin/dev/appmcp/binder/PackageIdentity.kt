package dev.appmcp.binder

import android.content.pm.PackageManager
import android.os.Build
import java.security.MessageDigest

/**
 * 调用方 uid → 包名与签名证书摘要（spec/naming.md 10.2、10.3：身份来自系统，不来自对方的自我声明）。
 *
 * 摘要格式 `sha256:<小写十六进制>`（与 App 登记文件的 `signature.fingerprint` 相同）。
 */
data class PackageIdentity(val uid: Int, val packages: List<String>, val certificates: Set<String>) {
    companion object {
        /** 查询 [uid] 名下的包与签名证书；查不到的包被跳过（不抛异常）。 */
        @JvmStatic
        fun of(pm: PackageManager, uid: Int): PackageIdentity {
            val packages = pm.getPackagesForUid(uid)?.toList().orEmpty()
            val certs = packages.flatMap { certificatesOf(pm, it) }.toSet()
            return PackageIdentity(uid, packages, certs)
        }

        /** 包当前的签名证书摘要（API 28+ 取签名轮换后的当前证书）。 */
        @JvmStatic
        @Suppress("DEPRECATION")
        fun certificatesOf(pm: PackageManager, packageName: String): List<String> = runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                val info = pm.getPackageInfo(packageName, PackageManager.GET_SIGNING_CERTIFICATES).signingInfo
                val signers = when {
                    info == null -> emptyArray()
                    info.hasMultipleSigners() -> info.apkContentsSigners
                    else -> info.signingCertificateHistory?.takeLast(1)?.toTypedArray() ?: emptyArray()
                }
                signers.map { digest(it.toByteArray()) }
            } else {
                pm.getPackageInfo(packageName, PackageManager.GET_SIGNATURES).signatures.orEmpty().map { digest(it.toByteArray()) }
            }
        }.getOrDefault(emptyList())

        /** `sha256:<小写十六进制>`。 */
        @JvmStatic
        fun digest(bytes: ByteArray): String =
            "sha256:" + MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

        /** 规范化用户填写的摘要：去掉 `:` / 空白、转小写，补 `sha256:` 前缀；不是 64 位十六进制时为 null。 */
        @JvmStatic
        fun normalizeDigest(text: String): String? {
            val hex = text.trim().removePrefix("sha256:").removePrefix("SHA256:").replace(":", "").replace(" ", "").lowercase()
            return if (hex.length == 64 && hex.all { it in '0'..'9' || it in 'a'..'f' }) "sha256:$hex" else null
        }
    }
}

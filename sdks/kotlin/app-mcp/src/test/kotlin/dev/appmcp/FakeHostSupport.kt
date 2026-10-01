package dev.appmcp

import java.io.File
import org.junit.jupiter.api.Assumptions.assumeTrue

/** 测试支持：定位 / 构建 crates/native 的 fake_host（`APP_MCP_FAKE_HOST` 优先，否则 cargo 构建到仓库 target/）。 */
internal object FakeHostSupport {
    val repoRoot: File = File(System.getProperty("appmcp.repoRoot") ?: "../../..")
    private val targetDir = File(System.getenv("CARGO_TARGET_DIR") ?: File(repoRoot, "target").canonicalPath)

    /** @error 没有 cargo 或构建失败时以 JUnit 假设失败跳过测试。 */
    val binary: File by lazy {
        System.getenv("APP_MCP_FAKE_HOST")?.let { return@lazy File(it) }
        val build = runCatching {
            ProcessBuilder("cargo", "build", "-q", "-p", "app-mcp-native", "--example", "fake_host")
                .directory(repoRoot)
                .redirectErrorStream(true)
                .start()
        }.getOrNull()
        assumeTrue(build != null, "没有 cargo，跳过集成测试")
        val log = build!!.inputStream.bufferedReader().readText()
        assumeTrue(build.waitFor() == 0, "fake_host 构建失败：$log")
        File(targetDir, "debug/examples/fake_host")
    }
}

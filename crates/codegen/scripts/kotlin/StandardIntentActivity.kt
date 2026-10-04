// verify.sh 使用：合并 intent-filter 片段的 Activity，演示 App 侧接入（parse 后按结果处理）。
package verify

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import appmcp.generated.hub.HubStandardIntentResult
import appmcp.generated.hub.HubStandardIntents

class StandardIntentActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        handle(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    private fun handle(intent: Intent) {
        when (val r = HubStandardIntents.parse(intent)) {
            is HubStandardIntentResult.Matched -> title = r.call.tool
            is HubStandardIntentResult.Invalid -> title = r.reason
            HubStandardIntentResult.Unrecognized -> finish()
        }
    }
}

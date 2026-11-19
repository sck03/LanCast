package dev.lancast.control

import org.json.JSONObject
import java.io.Closeable
import java.util.UUID

/** A scoped media grant on an existing core; closing it never stops the listener. */
class FileBridge(private val core: ControlSession) : Closeable {
    private var request: String? = null
    private var completed: ((Result<String>) -> Unit)? = null

    fun open(url: String, fingerprint: String, ready: (Result<String>) -> Unit) {
        close()
        val id = UUID.randomUUID().toString()
        request = id; completed = ready
        core.command("bridge.create", JSONObject().put("url", url).put("fingerprint", fingerprint).put("requestId", id))
    }

    fun onEvent(event: JSONObject): Boolean {
        val type = event.optString("type")
        val body = event.optJSONObject("body") ?: return false
        if (type != "bridge.ready" && !(type == "error" && body.optString("op").startsWith("bridge."))) return false
        if (request != null && body.optString("requestId") == request) {
            val callback = completed; completed = null
            callback?.invoke(if (type == "bridge.ready") Result.success(body.getString("url")) else Result.failure(IllegalStateException(body.optString("code"))))
        }
        return true
    }

    override fun close() {
        val id = request; request = null; completed = null
        if (id != null) core.command("bridge.close", JSONObject().put("requestId", id))
    }
}

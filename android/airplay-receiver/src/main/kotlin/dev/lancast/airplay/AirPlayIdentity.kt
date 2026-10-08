package dev.lancast.airplay

import android.content.Context
import android.util.AtomicFile
import java.io.File
import java.security.SecureRandom

internal object AirPlayIdentity {
    /** Never falls back to an example identity and never silently rotates a damaged identity. */
    fun seed(context: Context): ByteArray {
        val file = AtomicFile(File(context.noBackupFilesDir, "airplay-identity-v1"))
        if (file.baseFile.exists()) return file.openRead().use { it.readBytes() }.also { check(it.size == 32) { "AirPlay 身份文件损坏" } }
        val bytes = ByteArray(32).also { SecureRandom().nextBytes(it) }
        val output = file.startWrite()
        try { output.write(bytes); file.finishWrite(output) } catch (e: Exception) { file.failWrite(output); throw e }
        return bytes
    }
    fun pin(): String {
        val random = SecureRandom()
        while (true) {
            val pin = (1..8).map { random.nextInt(10) }.joinToString("")
            if (pin.toSet().size > 1 && pin != "12345678" && pin != "87654321") return pin
        }
    }
}

package dev.lancast.control

import okhttp3.OkHttpClient
import java.security.MessageDigest
import java.security.SecureRandom
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.X509TrustManager

/** Exact out-of-band SPKI binding for our self-signed LAN identity. Never a trust-all manager. */
fun pinnedHttp(fingerprint: String): OkHttpClient {
    val normalized = fingerprint.replace(":", "").lowercase()
    require(normalized.matches(Regex("[0-9a-f]{64}")))
    val expected = normalized.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
    val trust = object : X509TrustManager {
        override fun getAcceptedIssuers() = emptyArray<X509Certificate>()
        override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) =
            throw CertificateException("Client certificates not supported")
        override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
            if (chain.isEmpty() || !MessageDigest.isEqual(expected, MessageDigest.getInstance("SHA-256").digest(chain[0].publicKey.encoded)))
                throw CertificateException("TLS_PIN_MISMATCH")
        }
    }
    val ssl = SSLContext.getInstance("TLS")
    ssl.init(null, arrayOf(trust), SecureRandom())
    return OkHttpClient.Builder().sslSocketFactory(ssl.socketFactory, trust)
        // The verified SPKI is the identity; an IP-based LAN endpoint has no public DNS certificate.
        .hostnameVerifier { _, session ->
            val cert = session.peerCertificates.firstOrNull() as? X509Certificate
            cert != null && MessageDigest.isEqual(expected, MessageDigest.getInstance("SHA-256").digest(cert.publicKey.encoded))
        }.followRedirects(false).followSslRedirects(false)
        .connectTimeout(5, TimeUnit.SECONDS).readTimeout(15, TimeUnit.SECONDS).build()
}

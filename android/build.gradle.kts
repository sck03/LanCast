plugins {
    id("com.android.application") version "8.13.2" apply false
    id("com.android.library") version "8.13.2" apply false
    id("org.jetbrains.kotlin.android") version "2.3.10" apply false
    id("org.jetbrains.kotlin.jvm") version "2.3.10" apply false
}

val product = groovy.json.JsonSlurper().parse(rootDir.resolve("../build-config.json")) as Map<*, *>
val productVersion = System.getenv("LC_VERSION")?.takeIf { it.isNotEmpty() } ?: product["version"].toString()
val productBuild = System.getenv("LC_BUILD_NUMBER")?.takeIf { it.isNotEmpty() } ?: product["build_number"].toString()
val productAbis = System.getenv("LC_ANDROID_ABIS")?.takeIf { it.isNotEmpty() }?.split(",")
    ?: (product["android_abis"] as List<*>).map { it.toString() }
require(productVersion.matches(Regex("(0|[1-9][0-9]{0,3})\\.(0|[1-9][0-9]{0,3})\\.(0|[1-9][0-9]{0,3})"))) { "Invalid application version" }
require(productBuild.matches(Regex("[1-9][0-9]{0,4}")) && productBuild.toInt() <= 65535) { "Invalid application build number" }
require(productAbis.isNotEmpty() && productAbis.toSet().size == productAbis.size && productAbis.all { it in setOf("armeabi-v7a", "arm64-v8a") }) { "Unsupported Android ABI selection" }
extra["productVersion"] = productVersion
extra["productBuild"] = productBuild.toInt()
extra["productAbis"] = productAbis
